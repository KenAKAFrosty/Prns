use super::*;

#[test]
fn every_host_constructible_medium_maps() {
    let plan = plan_of(STOCK);
    assert_eq!(plan.interfaces.len(), 5);
    assert!(plan.deferred.is_empty());
    assert_eq!(
        named(&plan, "Default Interface").medium,
        PlannedMedium::AutoWifi { group: None }
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
            baud: 115200,
        }
    );
}

#[test]
fn tcp_socket_settings_are_typed_into_the_plan() {
    let plan = plan_of(
        "[interfaces]\n\
         [[Client]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = peer\ntarget_port = 4242\n\
         i2p_tunneled = Yes\nconnect_timeout = 11\nmax_reconnect_tries = 3\n\
         [[Server]]\ntype = TCPServerInterface\nenabled = Yes\nport = 4243\nprefer_ipv6 = Yes\n\
         i2p_tunneled = Yes\n",
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
            }
        }
    );
    assert!(named(&plan, "Client").unapplied.is_empty());
    assert!(named(&plan, "Server").unapplied.is_empty());
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
            baud: 115200,
            preamble_ms: 350,
            txtail_ms: 20,
            persistence: 64,
            slottime_ms: 20,
        }
    );
}

#[test]
fn a_kiss_tnc_carries_configured_timing_and_notes_what_it_cannot_honor() {
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
            baud: RNS_DEFAULT_SERIAL_BAUD,
            preamble_ms: 150,
            txtail_ms: 50,
            persistence: 200,
            slottime_ms: 30,
        }
    );
    assert!(tnc
        .unapplied
        .contains(&UnappliedSetting::MediumOption(interface_key::FLOW_CONTROL)));
    assert!(tnc
        .unapplied
        .contains(&UnappliedSetting::MediumOption(interface_key::ID_CALLSIGN)));
    assert!(tnc
        .unapplied
        .contains(&UnappliedSetting::MediumOption(interface_key::ID_INTERVAL)));
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
            baud: RNS_DEFAULT_SERIAL_BAUD,
            preamble_ms: 350,
            txtail_ms: 20,
            persistence: 64,
            slottime_ms: 20,
            callsign: "N0CALL".to_string(),
            ssid: 2,
        }
    );
}

#[test]
fn an_ax25_tnc_without_a_callsign_or_ssid_defers_with_the_missing_key() {
    let no_call = plan_of(
        "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\nssid = 0\n",
    );
    assert_eq!(
        no_call.deferred[0].why,
        DeferReason::MissingRequiredField {
            key: interface_key::CALLSIGN
        }
    );
    let no_ssid = plan_of(
        "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\ncallsign = N0CALL\n",
    );
    assert_eq!(
        no_ssid.deferred[0].why,
        DeferReason::MissingRequiredField {
            key: interface_key::SSID
        }
    );
}

#[test]
fn a_pipe_plans_with_its_command_and_the_default_respawn_delay() {
    let plan = plan_of(
        "[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\ncommand = nc -l 4242\n",
    );
    assert_eq!(
        named(&plan, "Subprocess").medium,
        PlannedMedium::Pipe {
            command: "nc -l 4242".to_string(),
            respawn_delay_ms: 5_000,
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
            command: "prog".to_string(),
            respawn_delay_ms: 2_500,
        }
    );
}

#[test]
fn a_pipe_without_a_command_defers_with_the_missing_key() {
    let plan = plan_of("[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\n");
    assert_eq!(
        plan.deferred[0].why,
        DeferReason::MissingRequiredField {
            key: interface_key::COMMAND
        }
    );
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
    assert!(spine.unapplied.is_empty());

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
    assert!(uplink.unapplied.is_empty());
}

#[test]
fn a_disabled_interface_is_skipped_before_planning() {
    let plan = plan_of(
        "[interfaces]\n[[Off]]\ntype = TCPClientInterface\ntarget_host = h\ntarget_port = 1\n",
    );
    assert!(plan.interfaces.is_empty());
    assert!(plan.deferred.is_empty());
}

#[test]
fn a_missing_required_field_is_invalid_before_planning() {
    let invalid =
        parse("[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\n");
    assert!(invalid.is_err());
}

#[test]
fn an_unconstructible_kind_fails_before_planning() {
    let errors =
        parse("[interfaces]\n[[Mesh]]\ntype = WeaveInterface\nenabled = Yes\nport = 4242\n")
            .unwrap_err();
    assert!(errors.diagnostics().iter().any(|diagnostic| {
        diagnostic.code() == crate::ConfigDiagnosticCode::UnsupportedInterface
    }));
}

#[test]
fn an_rnode_plans_with_its_radio_channel_and_scales_its_airtime_locks() {
    let plan = plan_of(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
         frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
         codingrate = 5\nairtime_limit_short = 1.5\nairtime_limit_long = 5.0\n",
    );
    assert_eq!(
        named(&plan, "Radio").medium,
        PlannedMedium::Rnode {
            device: "/dev/ttyUSB0".to_string(),
            frequency_hz: 868_000_000,
            bandwidth_hz: 125_000,
            txpower_dbm: 7,
            spreading_factor: 8,
            coding_rate: 5,
            airtime_limit_short_centi: Some(150),
            airtime_limit_long_centi: Some(500),
        }
    );
}

#[test]
fn an_rnode_without_a_radio_field_defers_with_the_missing_key() {
    let no_freq = plan_of(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
         bandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n",
    );
    assert!(no_freq.interfaces.is_empty());
    assert_eq!(
        no_freq.deferred[0].why,
        DeferReason::MissingRequiredField {
            key: interface_key::FREQUENCY
        }
    );
    let no_sf = plan_of(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
         frequency = 868000000\nbandwidth = 125000\ntxpower = 7\ncodingrate = 5\n",
    );
    assert_eq!(
        no_sf.deferred[0].why,
        DeferReason::MissingRequiredField {
            key: interface_key::SPREADINGFACTOR
        }
    );
}

#[test]
fn an_rnode_surfaces_flow_control_and_beaconing_as_unapplied() {
    let plan = plan_of(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
         frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
         codingrate = 5\nflow_control = Yes\nid_callsign = N0CALL\nid_interval = 600\n",
    );
    let radio = named(&plan, "Radio");
    assert!(radio
        .unapplied
        .contains(&UnappliedSetting::MediumOption(interface_key::FLOW_CONTROL)));
    assert!(radio
        .unapplied
        .contains(&UnappliedSetting::MediumOption(interface_key::ID_CALLSIGN)));
    assert!(radio
        .unapplied
        .contains(&UnappliedSetting::MediumOption(interface_key::ID_INTERVAL)));
}
