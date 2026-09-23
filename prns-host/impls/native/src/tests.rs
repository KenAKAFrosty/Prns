use super::*;
use prns_host::{DestinationName, PrnsLimits, SingleDestinationConfig};
use std::fs;
use std::sync::atomic::AtomicUsize;
use std::time::{SystemTime, UNIX_EPOCH};

struct Sink;

impl NativeEventSink for Sink {
    fn running(&self) {}

    fn publish_application(&self, _event: ApplicationEvent) -> bool {
        true
    }

    fn publish_resource(&self, _event: ResourceAvailable, _body: Vec<u8>) -> bool {
        true
    }

    fn publish_diagnostic(&self, _event: DiagnosticEvent) {}

    fn stopped(&self) {}

    fn failed(&self, _detail: String) {}
}

struct RecordingSink {
    diagnostics: Mutex<Vec<DiagnosticEvent>>,
    changed: Condvar,
}

impl RecordingSink {
    fn new() -> Self {
        Self {
            diagnostics: Mutex::new(Vec::new()),
            changed: Condvar::new(),
        }
    }

    fn wait_for(&self, predicate: impl Fn(&DiagnosticEvent) -> bool) -> bool {
        let diagnostics = lock(&self.diagnostics);
        let waited = self
            .changed
            .wait_timeout_while(diagnostics, Duration::from_secs(2), |events| {
                !events.iter().any(&predicate)
            })
            .unwrap_or_else(PoisonError::into_inner);
        waited.0.iter().any(predicate)
    }

    fn diagnostics(&self) -> Vec<DiagnosticEvent> {
        lock(&self.diagnostics).clone()
    }
}

impl NativeEventSink for RecordingSink {
    fn running(&self) {}

    fn publish_application(&self, _event: ApplicationEvent) -> bool {
        true
    }

    fn publish_resource(&self, _event: ResourceAvailable, _body: Vec<u8>) -> bool {
        true
    }

    fn publish_diagnostic(&self, event: DiagnosticEvent) {
        lock(&self.diagnostics).push(event);
        self.changed.notify_all();
    }

    fn stopped(&self) {}

    fn failed(&self, _detail: String) {}
}

fn config() -> HostConfig {
    HostConfig {
        identity: IdentityConfig::GenerateEphemeral,
        persistence: PersistenceConfig::Ephemeral,
        role: HostRole::Endpoint,
        destinations: Vec::new(),
        required_capabilities: Vec::new(),
        limits: PrnsLimits::balanced(),
    }
}

fn temporary_root(label: &str) -> Result<std::path::PathBuf, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    Ok(std::env::temp_dir().join(format!(
        "prns-host-native-{label}-{}-{}",
        std::process::id(),
        now.as_nanos()
    )))
}

fn persistent_config(root: &Path) -> HostConfig {
    persistent_endpoint(root, Vec::new(), Vec::new(), PrnsLimits::balanced())
}

fn serial_line() -> prns_host::SerialLineConfig {
    prns_host::SerialLineConfig {
        baud: 115_200,
        data_bits: prns_host::SerialDataBits::Eight,
        parity: prns_host::SerialParity::None,
        stop_bits: prns_host::SerialStopBits::One,
    }
}

fn radio() -> prns_host::RNodeRadioConfig {
    prns_host::RNodeRadioConfig {
        frequency_hz: 915_000_000,
        bandwidth_hz: 125_000,
        tx_power_dbm: 14,
        spreading_factor: 8,
        coding_rate: 5,
    }
}

#[test]
fn backend_info_reports_every_compiled_stable_interface() {
    let backend = native_backend_info();
    for kind in native_interface_kinds() {
        assert!(backend.supports_interface(*kind));
    }
    assert!(!backend.supports_interface(InterfaceKind::BrowserRendezvous));
    assert_eq!(native_interface_kinds().len(), 18);
}

#[test]
fn every_supported_typed_interface_uses_the_effective_planner() -> Result<(), String> {
    let configs = vec![
        InterfaceConfig::AutoLan {
            group_id: Some("sdk-group".to_string()),
            discovery_scope: Some(prns_host::DiscoveryScope::Organization),
            discovery_port: Some(29_710),
            data_port: Some(42_444),
            devices: vec!["eth0".to_string()],
            ignored_devices: vec!["lo".to_string()],
            multicast_address_type: Some(prns_host::MulticastAddressType::Permanent),
        },
        InterfaceConfig::TcpClient {
            target: "127.0.0.1:4242".to_string(),
            bitrate: Bitrate::BitsPerSecond(1_000_000),
        },
        InterfaceConfig::TcpServer {
            bind: "[::1]:4242".to_string(),
            bitrate: Bitrate::Auto,
        },
        InterfaceConfig::Udp {
            local: "0.0.0.0:4242".to_string(),
            peer: "127.0.0.1:4243".to_string(),
            bitrate: Bitrate::BitsPerSecond(2_000_000),
        },
        InterfaceConfig::Serial {
            port: "/dev/ttyUSB0".to_string(),
            line: serial_line(),
        },
        InterfaceConfig::Kiss {
            port: "/dev/ttyUSB0".to_string(),
            line: serial_line(),
            flow_control: true,
            preamble_millis: 150,
            transmit_tail_millis: 20,
            persistence: 64,
            slot_time_millis: 20,
            station_callsign: Some("N0CALL".to_string()),
            station_interval_seconds: Some(600),
        },
        InterfaceConfig::Ax25Kiss {
            port: "/dev/ttyUSB0".to_string(),
            line: serial_line(),
            flow_control: true,
            preamble_millis: 150,
            transmit_tail_millis: 20,
            persistence: 64,
            slot_time_millis: 20,
            callsign: "N0CALL".to_string(),
            ssid: 1,
        },
        InterfaceConfig::RNode {
            port: "/dev/ttyUSB0".to_string(),
            radio: radio(),
            flow_control: true,
            station_callsign: Some("N0CALL".to_string()),
            station_interval_seconds: Some(600),
            airtime_limit_short_centi_percent: Some(125),
            airtime_limit_long_centi_percent: Some(250),
        },
        InterfaceConfig::MultiRNode {
            port: "/dev/ttyUSB0".to_string(),
            station_callsign: Some("N0CALL".to_string()),
            station_interval_seconds: Some(600),
            members: vec![prns_host::MultiRNodeMemberConfig {
                name: "uplink".to_string(),
                virtual_port: 1,
                radio: radio(),
                flow_control: true,
                outgoing: true,
            }],
        },
        InterfaceConfig::Pipe {
            command: vec![
                "printf".to_string(),
                "two words".to_string(),
                "single'quote".to_string(),
            ],
            respawn_delay_millis: 1_500,
        },
        InterfaceConfig::BackboneClient {
            target: "backbone.example:4242".to_string(),
            bitrate: Bitrate::Auto,
        },
        InterfaceConfig::BackboneServer {
            bind: "0.0.0.0:4242".to_string(),
            bitrate: Bitrate::Auto,
        },
        InterfaceConfig::I2p {
            peers: vec!["example.i2p".to_string()],
            connectable: false,
        },
        InterfaceConfig::Weave {
            port: "/dev/ttyACM0".to_string(),
        },
        InterfaceConfig::AutomaticUsb,
        InterfaceConfig::AutomaticBluetoothLe,
        InterfaceConfig::WebSocketClient {
            target: "ws://127.0.0.1:4242".to_string(),
            framing: prns_host::WebSocketFramingSelection::Auto,
        },
        InterfaceConfig::WebSocketServer {
            bind: "127.0.0.1:4242".to_string(),
            framing: prns_host::WebSocketFramingSelection::Hdlc,
        },
    ];
    for config in configs {
        config.validate().map_err(|error| format!("{error:?}"))?;
        let plan = typed_interface_plan(&config, None).map_err(|error| format!("{error:?}"))?;
        if plan.interfaces.is_empty() {
            return Err(format!(
                "{:?} produced no planned interfaces",
                config.kind()
            ));
        }
    }
    assert!(matches!(
        typed_interface_plan(
            &InterfaceConfig::BrowserRendezvous {
                url: "ws://127.0.0.1:4242".to_string(),
            },
            None
        ),
        Err(CommandFailure::UnsupportedByBackend)
    ));
    Ok(())
}

#[test]
fn typed_interface_routing_is_applied_before_attachment() -> Result<(), String> {
    let config = InterfaceConfig::TcpClient {
        target: "127.0.0.1:4242".to_string(),
        bitrate: Bitrate::Auto,
    };
    let default_plan = typed_interface_plan(&config, None).map_err(|error| format!("{error:?}"))?;
    let default_policy = default_plan
        .interfaces
        .first()
        .ok_or_else(|| "default interface plan was empty".to_string())?
        .policy;
    if default_policy.mode != personal_rns::interfaces::InterfaceMode::Full
        || default_policy.gravity
            != personal_rns::interfaces::InterfaceGravity::from_bitrate(default_policy.bitrate)
        || default_policy.common.forwarding.recursive_path_requests
            != personal_rns::interfaces::RecursivePathRequestPolicy::InheritNode
        || !default_policy.common.forwarding.announces_from_internal
        || default_policy.common.forwarding.announces_to_internal
    {
        return Err(format!(
            "unexpected default routing policy: {default_policy:?}"
        ));
    }
    let routing = InterfaceRoutingPolicy {
        mode: Some(InterfaceMode::Boundary),
        gravity: Some(-73),
        recursive_path_requests: Some(true),
        announces_from_internal: Some(false),
        announces_to_internal: Some(true),
    };
    let plan =
        typed_interface_plan(&config, Some(routing)).map_err(|error| format!("{error:?}"))?;
    let policy = plan
        .interfaces
        .first()
        .ok_or_else(|| "interface plan was empty".to_string())?
        .policy;
    if policy.mode != personal_rns::interfaces::InterfaceMode::Boundary
        || policy.gravity.get() != -73
        || policy.common.forwarding.recursive_path_requests
            != personal_rns::interfaces::RecursivePathRequestPolicy::Enabled
        || policy.common.forwarding.announces_from_internal
        || !policy.common.forwarding.announces_to_internal
    {
        return Err(format!("unexpected routing policy: {policy:?}"));
    }
    let multi_plan = typed_interface_plan(
        &InterfaceConfig::MultiRNode {
            port: "/dev/ttyUSB0".to_string(),
            station_callsign: None,
            station_interval_seconds: None,
            members: vec![
                prns_host::MultiRNodeMemberConfig {
                    name: "uplink".to_string(),
                    virtual_port: 1,
                    radio: radio(),
                    flow_control: true,
                    outgoing: true,
                },
                prns_host::MultiRNodeMemberConfig {
                    name: "downlink".to_string(),
                    virtual_port: 2,
                    radio: radio(),
                    flow_control: true,
                    outgoing: true,
                },
            ],
        },
        Some(routing),
    )
    .map_err(|error| format!("{error:?}"))?;
    if multi_plan.interfaces.is_empty()
        || multi_plan.interfaces.iter().any(|interface| {
            let policy = interface.policy;
            policy.mode != personal_rns::interfaces::InterfaceMode::Boundary
                || policy.gravity.get() != -73
                || policy.common.forwarding.recursive_path_requests
                    != personal_rns::interfaces::RecursivePathRequestPolicy::Enabled
                || policy.common.forwarding.announces_from_internal
                || !policy.common.forwarding.announces_to_internal
        })
    {
        return Err(format!(
            "multi-interface routing was not inherited: {:?}",
            multi_plan.interfaces
        ));
    }
    assert!(matches!(
        typed_interface_plan(
            &config,
            Some(InterfaceRoutingPolicy {
                gravity: Some(SAFE_INT_MAX + 1),
                ..routing
            })
        ),
        Err(CommandFailure::InvalidConfiguration { .. })
    ));
    Ok(())
}

#[test]
fn command_completion_and_interruption_signal_readiness() {
    let readiness_count = Arc::new(AtomicUsize::new(0));
    let callback_count = Arc::clone(&readiness_count);
    let completion = Arc::new(CommandCompletion::new(Some(Arc::new(move || {
        callback_count.fetch_add(1, Ordering::AcqRel);
    }))));
    let command = CommandHandle {
        completion: Arc::clone(&completion),
    };
    completion.finish(Ok(CommandOutcome::Announced));
    assert_eq!(readiness_count.load(Ordering::Acquire), 1);
    command.interrupt_wait();
    assert_eq!(readiness_count.load(Ordering::Acquire), 2);
}

#[cfg(unix)]
fn supplied_pipe_config(aspect: &str) -> Result<HostConfig, String> {
    Ok(HostConfig {
        destinations: vec![DestinationConfig::Single(SingleDestinationConfig {
            name: DestinationName::try_new("suppliedpipe", vec![aspect.to_string()])
                .map_err(|error| format!("{error:?}"))?,
            identity: DestinationIdentityConfig::HostIdentity,
            announce_app_data: Vec::new(),
            maximum_request_bytes: None,
            proof: DestinationProofStrategy::ProveAll,
            link_requests: DestinationLinkRequestPolicy::AcceptAll,
            ratchet: DestinationRatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::Refuse,
            request_handlers: Vec::new(),
        })],
        ..config()
    })
}

#[cfg(unix)]
fn attach_supplied_wire(
    host: &NativeHost,
    name: &str,
    wire: std::os::unix::net::UnixStream,
) -> Result<(NativeSuppliedPipe, InterfaceId), String> {
    wire.set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let pipe = host
        .begin_supplied_pipe(
            SuppliedPipeConfig {
                name: name.to_string(),
                respawn_delay: Duration::from_millis(50),
                bitrate: Bitrate::Auto,
            },
            None,
            None,
        )
        .map_err(|error| format!("{error:?}"))?;
    let command = pipe
        .claim_attachment()
        .ok_or_else(|| "the attachment command was already claimed".to_string())?;
    let interface = match command.wait(Some(Duration::from_secs(2))) {
        CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { interface })) => interface,
        other => return Err(format!("{other:?}")),
    };
    let request = match pipe.next_request(Some(Duration::from_secs(2))) {
        SuppliedPipeRequestWait::Request(request) => request,
        _ => return Err("the supplied pipe did not request a descriptor".to_string()),
    };
    if !request.provide(std::os::fd::OwnedFd::from(wire)) {
        return Err("the supplied pipe rejected its descriptor".to_string());
    }
    Ok((pipe, interface))
}

#[cfg(unix)]
fn wait_until_connected(host: &NativeHost, interface: InterfaceId) -> Result<(), String> {
    use prns_host::InterfaceHealth;

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snapshot = host
            .snapshot(Some(Duration::from_secs(2)))
            .map_err(|error| format!("{error:?}"))?;
        if snapshot.interfaces.iter().any(|entry| {
            entry.interface_id == interface
                && entry.kind == Some(InterfaceKind::Pipe)
                && entry.health == InterfaceHealth::Connected
        }) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("supplied pipe never connected: {snapshot:?}"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
#[test]
fn supplied_pipes_keep_pipe_identity_and_carry_announces() -> Result<(), String> {
    let (first_wire, second_wire) =
        std::os::unix::net::UnixStream::pair().map_err(|error| error.to_string())?;
    let first_sink = Arc::new(RecordingSink::new());
    let first = NativeHost::start(supplied_pipe_config("first")?, first_sink)
        .map_err(|error| format!("{error:?}"))?;
    let second_sink = Arc::new(RecordingSink::new());
    let second = NativeHost::start(supplied_pipe_config("second")?, second_sink.clone())
        .map_err(|error| format!("{error:?}"))?;

    let (_first_pipe, first_interface) = attach_supplied_wire(&first, "to-second", first_wire)?;
    let (_second_pipe, second_interface) = attach_supplied_wire(&second, "to-first", second_wire)?;
    wait_until_connected(&first, first_interface)?;
    wait_until_connected(&second, second_interface)?;
    if first_interface.as_bytes()[0] != personal_rns::interfaces::InterfaceKind::Pipe as u8 {
        return Err("supplied pipe minted a non-Pipe engine identity".to_string());
    }

    let destination = *first
        .destination_hashes()
        .first()
        .ok_or_else(|| "the first host has no destination".to_string())?;
    let announced = first
        .submit(HostCommand::Announce {
            destination,
            interface: Some(first_interface),
        })
        .map_err(|error| format!("{error:?}"))?;
    if !matches!(
        announced.wait(Some(Duration::from_secs(2))),
        CommandWait::Completed(Ok(CommandOutcome::Announced))
    ) {
        return Err("the first host did not announce".to_string());
    }
    if !second_sink.wait_for(|event| {
        matches!(
            event,
            DiagnosticEvent::AnnounceHeard { destination: heard, .. }
                if *heard == destination
        )
    }) {
        return Err(format!(
            "the second host never heard the announce: {:?}",
            second_sink.diagnostics()
        ));
    }

    first.stop();
    second.stop();
    Ok(())
}

#[cfg(unix)]
#[test]
fn supplied_pipe_asks_for_a_fresh_descriptor_after_disconnect() -> Result<(), String> {
    let (first_wire, first_peer) =
        std::os::unix::net::UnixStream::pair().map_err(|error| error.to_string())?;
    let (second_wire, second_peer) =
        std::os::unix::net::UnixStream::pair().map_err(|error| error.to_string())?;
    first_wire
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    second_wire
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let host = NativeHost::start(config(), Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
    let pipe = host
        .begin_supplied_pipe(
            SuppliedPipeConfig {
                name: "reconnecting".to_string(),
                respawn_delay: Duration::from_millis(25),
                bitrate: Bitrate::Auto,
            },
            None,
            None,
        )
        .map_err(|error| format!("{error:?}"))?;
    let attached = pipe
        .claim_attachment()
        .ok_or_else(|| "the attachment command was already claimed".to_string())?;
    let interface = match attached.wait(Some(Duration::from_secs(2))) {
        CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { interface })) => interface,
        other => return Err(format!("{other:?}")),
    };
    let first_request = match pipe.next_request(Some(Duration::from_secs(2))) {
        SuppliedPipeRequestWait::Request(request) => request,
        _ => return Err("the first descriptor was not requested".to_string()),
    };
    if !first_request.provide(std::os::fd::OwnedFd::from(first_wire)) {
        return Err("the first descriptor was rejected".to_string());
    }
    wait_until_connected(&host, interface)?;
    drop(first_peer);
    let second_request = match pipe.next_request(Some(Duration::from_secs(2))) {
        SuppliedPipeRequestWait::Request(request) => request,
        _ => return Err("the replacement descriptor was not requested".to_string()),
    };
    if !second_request.provide(std::os::fd::OwnedFd::from(second_wire)) {
        return Err("the replacement descriptor was rejected".to_string());
    }
    wait_until_connected(&host, interface)?;
    drop(second_peer);
    host.stop();
    Ok(())
}

#[cfg(unix)]
#[test]
fn supplied_pipe_controller_release_detaches_the_interface() -> Result<(), String> {
    let host = NativeHost::start(config(), Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
    let pipe = host
        .begin_supplied_pipe(
            SuppliedPipeConfig {
                name: "owned-controller".to_string(),
                respawn_delay: Duration::from_millis(25),
                bitrate: Bitrate::Auto,
            },
            None,
            None,
        )
        .map_err(|error| format!("{error:?}"))?;
    let attached = pipe
        .claim_attachment()
        .ok_or_else(|| "the attachment command was already claimed".to_string())?;
    let interface = match attached.wait(Some(Duration::from_secs(2))) {
        CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { interface })) => interface,
        other => return Err(format!("{other:?}")),
    };
    pipe.close();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snapshot = host
            .snapshot(Some(Duration::from_secs(2)))
            .map_err(|error| format!("{error:?}"))?;
        if snapshot
            .interfaces
            .iter()
            .all(|entry| entry.interface_id != interface)
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err("the controller release left its interface attached".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(
        pipe.next_request(Some(Duration::ZERO)),
        SuppliedPipeRequestWait::Stopped
    ));
    host.stop();
    Ok(())
}

#[cfg(unix)]
#[test]
fn explicit_detach_stops_the_supplied_pipe_controller() -> Result<(), String> {
    let host = NativeHost::start(config(), Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
    let pipe = host
        .begin_supplied_pipe(
            SuppliedPipeConfig {
                name: "explicit-detach".to_string(),
                respawn_delay: Duration::from_millis(25),
                bitrate: Bitrate::Auto,
            },
            None,
            None,
        )
        .map_err(|error| format!("{error:?}"))?;
    let attached = pipe
        .claim_attachment()
        .ok_or_else(|| "the attachment command was already claimed".to_string())?;
    let interface = match attached.wait(Some(Duration::from_secs(2))) {
        CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { interface })) => interface,
        other => return Err(format!("{other:?}")),
    };
    let detached = host
        .submit(HostCommand::DetachInterface { interface })
        .map_err(|error| format!("{error:?}"))?;
    if !matches!(
        detached.wait(Some(Duration::from_secs(2))),
        CommandWait::Completed(Ok(CommandOutcome::InterfaceDetached { .. }))
    ) {
        return Err("the explicit detach did not settle successfully".to_string());
    }
    assert!(matches!(
        pipe.next_request(Some(Duration::ZERO)),
        SuppliedPipeRequestWait::Stopped
    ));
    host.stop();
    Ok(())
}

#[test]
fn native_host_executes_interface_commands() -> Result<(), String> {
    let host = NativeHost::start(config(), Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
    let command = host
        .submit(HostCommand::AttachTcpClient {
            target: "127.0.0.1:9".to_string(),
            bitrate: Bitrate::Auto,
        })
        .map_err(|error| format!("{error:?}"))?;
    let attached = match command.wait(Some(Duration::from_secs(2))) {
        CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { interface })) => interface,
        other => return Err(format!("{other:?}")),
    };
    let command = host
        .submit(HostCommand::DetachInterface {
            interface: attached,
        })
        .map_err(|error| format!("{error:?}"))?;
    if !matches!(
        command.wait(Some(Duration::from_secs(2))),
        CommandWait::Completed(Ok(CommandOutcome::InterfaceDetached { .. }))
    ) {
        return Err("interface did not detach".to_string());
    }
    let command = host
        .submit(HostCommand::SendChannelMessage {
            link_id: LinkId::new([0; 16]),
            message_type: 0xf000,
            payload: Vec::new(),
        })
        .map_err(|error| format!("{error:?}"))?;
    if !matches!(
        command.wait(Some(Duration::from_secs(2))),
        CommandWait::Completed(Err(CommandFailure::InvalidChannelMessageType))
    ) {
        return Err("reserved channel type did not settle as invalid".to_string());
    }
    host.stop();
    Ok(())
}

#[test]
fn snapshots_track_interface_changes_consistently() -> Result<(), String> {
    let host = NativeHost::start(config(), Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
    let initial = host
        .snapshot(Some(Duration::from_secs(2)))
        .map_err(|error| format!("{error:?}"))?;
    if initial.revision != 1
        || !initial.interfaces.is_empty()
        || !initial.routes.is_empty()
        || initial.active_link_count != 0
        || initial.runtime.interface_count != 0
    {
        return Err("initial snapshot was inconsistent".to_string());
    }
    let attached = host
        .submit(HostCommand::AttachInterface {
            config: InterfaceConfig::TcpClient {
                target: "127.0.0.1:9".to_string(),
                bitrate: Bitrate::Auto,
            },
            routing: None,
        })
        .map_err(|error| format!("{error:?}"))?;
    let interface = match attached.wait(Some(Duration::from_secs(2))) {
        CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { interface })) => interface,
        other => return Err(format!("{other:?}")),
    };
    let attached_snapshot = host
        .snapshot(Some(Duration::from_secs(2)))
        .map_err(|error| format!("{error:?}"))?;
    if attached_snapshot.revision != 2
        || attached_snapshot.interfaces.len() != 1
        || attached_snapshot.interfaces[0].interface_id != interface
        || attached_snapshot.interfaces[0].kind != Some(InterfaceKind::TcpClient)
        || attached_snapshot.runtime.interface_count != 1
    {
        return Err("attached interface snapshot was inconsistent".to_string());
    }
    let detached = host
        .submit(HostCommand::DetachInterface { interface })
        .map_err(|error| format!("{error:?}"))?;
    if !matches!(
        detached.wait(Some(Duration::from_secs(2))),
        CommandWait::Completed(Ok(CommandOutcome::InterfaceDetached { .. }))
    ) {
        return Err("interface did not detach".to_string());
    }
    let detached_snapshot = host
        .snapshot(Some(Duration::from_secs(2)))
        .map_err(|error| format!("{error:?}"))?;
    if detached_snapshot.revision != 3
        || !detached_snapshot.interfaces.is_empty()
        || detached_snapshot.runtime.interface_count != 0
    {
        return Err("detached interface snapshot was inconsistent".to_string());
    }
    host.stop();
    Ok(())
}

#[test]
fn oversized_response_failure_remains_typed() {
    assert_eq!(
        request_failure(SendError::Failed(SendRequestFailure::ResponseTooLarge)),
        CommandFailure::ResponseTooLarge
    );
}

#[test]
fn response_resource_capacity_uses_the_existing_table_full_outcome() {
    assert_eq!(
        request_failure(SendError::Failed(SendRequestFailure::ResourceCapacity)),
        CommandFailure::ResourceTableFull
    );
}

#[test]
fn every_response_transfer_failure_reaches_its_stable_host_outcome() {
    let cases = [
        (
            ResourceFailureCause::CancelledBySender,
            CommandFailure::ResponseCancelledBySender,
        ),
        (
            ResourceFailureCause::RefusedHashmapUpdate(ApplyHashmapUpdateError::BeyondPartCount),
            CommandFailure::ResponseHashmapBeyondPartCount,
        ),
        (
            ResourceFailureCause::RefusedHashmapUpdate(ApplyHashmapUpdateError::SkipsAhead),
            CommandFailure::ResponseHashmapSkipsAhead,
        ),
        (
            ResourceFailureCause::RefusedHashmapUpdate(ApplyHashmapUpdateError::HashmapTooLong),
            CommandFailure::ResponseHashmapTooLong,
        ),
        (
            ResourceFailureCause::RefusedHashmapUpdate(ApplyHashmapUpdateError::HashmapRagged),
            CommandFailure::ResponseHashmapRagged,
        ),
        (
            ResourceFailureCause::RetriesExhausted,
            CommandFailure::ResponseRetriesExhausted,
        ),
        (
            ResourceFailureCause::LinkVanished,
            CommandFailure::ResponseLinkVanished,
        ),
        (
            ResourceFailureCause::TransferUnopenable,
            CommandFailure::ResponseTransferUnopenable,
        ),
        (
            ResourceFailureCause::TransferCorrupt,
            CommandFailure::ResponseTransferCorrupt,
        ),
        (
            ResourceFailureCause::ProofUnsendable,
            CommandFailure::ResponseProofUnsendable,
        ),
        (
            ResourceFailureCause::DecompressionFailed,
            CommandFailure::ResponseDecompressionFailed,
        ),
        (
            ResourceFailureCause::DecompressionTimedOut,
            CommandFailure::ResponseDecompressionTimedOut,
        ),
        (
            ResourceFailureCause::OpenTimedOut,
            CommandFailure::ResponseOpenTimedOut,
        ),
        (
            ResourceFailureCause::MetadataOverrun,
            CommandFailure::ResponseMetadataOverrun,
        ),
    ];

    for (cause, expected) in cases {
        assert_eq!(
            request_failure(SendError::Failed(
                SendRequestFailure::ResponseTransferFailed(cause),
            )),
            expected,
        );
    }
}

#[test]
fn request_byte_limits_stay_in_the_interoperable_integer_range() {
    assert!(is_optional_safe_uint(None));
    assert!(is_optional_safe_uint(Some(SAFE_UINT_MAX)));
    assert!(!is_optional_safe_uint(Some(SAFE_UINT_MAX + 1)));
}

#[test]
fn configured_host_registers_request_handlers() -> Result<(), String> {
    let mut config = config();
    config
        .destinations
        .push(DestinationConfig::Single(SingleDestinationConfig {
            name: DestinationName::try_new("host-test", ["request".to_string()])
                .map_err(|error| format!("{error:?}"))?,
            identity: DestinationIdentityConfig::HostIdentity,
            announce_app_data: Vec::new(),
            maximum_request_bytes: Some(4_096),
            proof: DestinationProofStrategy::ProveAll,
            link_requests: DestinationLinkRequestPolicy::AcceptAll,
            ratchet: DestinationRatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::Refuse,
            request_handlers: vec![RequestHandlerConfig {
                path: "/echo".to_string(),
                policy: RequestPolicy::AllowList,
            }],
        }));
    let host = NativeHost::start(config, Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
    let destination = host.destination_hashes()[0];
    let engine_path = personal_rns::routing::request_handlers::RequestPathHash::of("/echo");
    let command = host
        .submit(HostCommand::AllowRequester {
            destination,
            path_hash: RequestPathHash::new(*engine_path.as_bytes()),
            identity: IdentityHash::new([0x44; 16]),
        })
        .map_err(|error| format!("{error:?}"))?;
    if !matches!(
        command.wait(Some(Duration::from_secs(2))),
        CommandWait::Completed(Ok(CommandOutcome::RequesterAllowed))
    ) {
        return Err("requester was not admitted".to_string());
    }
    host.stop();
    Ok(())
}

#[test]
fn persistent_host_restores_identity_and_flushes_on_shutdown() -> Result<(), String> {
    let root = temporary_root("restart")?;
    let first_sink = Arc::new(RecordingSink::new());
    let first = NativeHost::start(persistent_config(&root), first_sink.clone())
        .map_err(|error| format!("{error:?}"))?;
    let identity = first.identity_hash();
    if !first_sink.wait_for(|event| matches!(event, DiagnosticEvent::PersistenceRestored { .. })) {
        return Err("first host did not report persistence restoration".to_string());
    }
    first.stop();
    if !first_sink.diagnostics().iter().any(|event| {
        matches!(
            event,
            DiagnosticEvent::PersistenceFlushed {
                cause: PersistenceFlushCause::Shutdown,
                ..
            }
        )
    }) {
        return Err("first host did not flush persistence during shutdown".to_string());
    }

    let second_sink = Arc::new(RecordingSink::new());
    let second = NativeHost::start(persistent_config(&root), second_sink.clone())
        .map_err(|error| format!("{error:?}"))?;
    if second.identity_hash() != identity {
        return Err("persistent host identity changed across restart".to_string());
    }
    if !second_sink.wait_for(|event| matches!(event, DiagnosticEvent::PersistenceRestored { .. })) {
        return Err("second host did not report persistence restoration".to_string());
    }
    second.stop();
    fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
    Ok(())
}

#[test]
fn persistence_path_must_be_a_directory() -> Result<(), String> {
    let root = temporary_root("not-directory")?;
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let state = root.join("state");
    fs::write(&state, b"not a directory").map_err(|error| error.to_string())?;
    let result = NativeHost::start(persistent_config(&root), Arc::new(Sink));
    let expected = NativeStartError::Persistence(PersistenceStartError::NotDirectory {
        path: state.to_string_lossy().into_owned(),
    });
    if result.err() != Some(expected) {
        return Err("non-directory persistence path was not typed".to_string());
    }
    fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
    Ok(())
}

fn isolated_upload(declared_length: u64) -> (NativeUpload, UploadSource) {
    let completion = Arc::new(CommandCompletion::new(None));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (chunks, source) = mpsc::channel(UPLOAD_CHUNK_CAPACITY);
    (
        NativeUpload {
            chunks: Mutex::new(Some(chunks)),
            completion,
            cancelled,
            declared_length,
            written: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            changed: watch::channel(()).0,
        },
        UploadSource::new(source),
    )
}

#[test]
fn upload_reports_overrun_and_early_eof() {
    let (overrun, _source) = isolated_upload(1);
    assert_eq!(overrun.write(&[1, 2]), Err(UploadWriteError::LengthOverrun));
    assert!(matches!(
        overrun.finish().wait(Some(Duration::ZERO)),
        CommandWait::Completed(Err(CommandFailure::ResourceLengthOverrun))
    ));

    let (early, _source) = isolated_upload(2);
    assert_eq!(early.write(&[1]), Ok(()));
    assert!(matches!(
        early.finish().wait(Some(Duration::ZERO)),
        CommandWait::Completed(Err(CommandFailure::ResourceEarlyEof))
    ));
}

#[test]
fn upload_is_bounded_and_release_cancels() {
    let (upload, _source) = isolated_upload(16);
    for _ in 0..UPLOAD_CHUNK_CAPACITY {
        assert_eq!(upload.write(&[1]), Ok(()));
    }
    assert_eq!(upload.write(&[1]), Err(UploadWriteError::WouldBlock));
    let command = CommandHandle {
        completion: Arc::clone(&upload.completion),
    };
    drop(upload);
    assert!(matches!(
        command.wait(Some(Duration::ZERO)),
        CommandWait::Completed(Err(CommandFailure::ResourceUploadCancelled))
    ));
}

#[test]
fn upload_rejects_oversized_chunks() {
    let (upload, _source) = isolated_upload((MAX_UPLOAD_CHUNK_BYTES + 1) as u64);
    let chunk = vec![0; MAX_UPLOAD_CHUNK_BYTES + 1];
    assert_eq!(upload.write(&chunk), Err(UploadWriteError::ChunkTooLarge));
}

#[test]
fn persistent_directory_is_exclusive_until_joined_stop() -> Result<(), String> {
    let root = temporary_root("exclusive-owner")?;
    let first = NativeHost::start(persistent_config(&root), Arc::new(Sink))
        .map_err(|e| format!("{e:?}"))?;
    let second = NativeHost::start(persistent_config(&root), Arc::new(Sink));
    assert!(matches!(
        second,
        Err(NativeStartError::Persistence(
            PersistenceStartError::Unavailable { .. }
        ))
    ));
    first.stop_result().map_err(|e| format!("{e:?}"))?;
    // The stopped Rust object remains alive; ownership ends at joined stop, not GC.
    let third = NativeHost::start(persistent_config(&root), Arc::new(Sink))
        .map_err(|e| format!("{e:?}"))?;
    third.stop_result().map_err(|e| format!("{e:?}"))?;
    fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn command_async_wait_is_runtime_independent_and_cancellation_keeps_command() {
    let completion = Arc::new(CommandCompletion::new(None));
    let command = CommandHandle {
        completion: Arc::clone(&completion),
    };
    let mut context = Context::from_waker(std::task::Waker::noop());
    let mut cancelled = Box::pin(command.wait_async());
    assert!(matches!(
        cancelled.as_mut().poll(&mut context),
        Poll::Pending
    ));
    drop(cancelled);
    completion.finish(Ok(CommandOutcome::Announced));
    let mut second = Box::pin(command.wait_async());
    assert!(matches!(
        second.as_mut().poll(&mut context),
        Poll::Ready(CommandWait::Completed(Ok(CommandOutcome::Announced)))
    ));
}

#[test]
fn cancelled_upload_write_does_not_reserve_declared_bytes() {
    let (upload, _source) = isolated_upload(16);
    for _ in 0..UPLOAD_CHUNK_CAPACITY {
        assert_eq!(upload.write(&[1]), Ok(()));
    }
    let mut context = Context::from_waker(std::task::Waker::noop());
    let mut write = Box::pin(upload.write_async(vec![9]));
    assert!(matches!(write.as_mut().poll(&mut context), Poll::Pending));
    drop(write);
    assert_eq!(
        upload.written.load(Ordering::Acquire),
        UPLOAD_CHUNK_CAPACITY as u64
    );
    let mut write = Box::pin(upload.write_async(vec![9]));
    assert!(matches!(write.as_mut().poll(&mut context), Poll::Pending));
    upload.abort();
    assert!(matches!(
        write.as_mut().poll(&mut context),
        Poll::Ready(Err(UploadWriteError::Finished))
    ));
}

#[test]
fn concurrent_native_stop_waiters_do_not_return_before_worker_join() -> Result<(), String> {
    struct PausedStop {
        gate: Mutex<(bool, bool)>,
        changed: Condvar,
    }
    impl NativeEventSink for PausedStop {
        fn running(&self) {}
        fn publish_application(&self, _: ApplicationEvent) -> bool {
            true
        }
        fn publish_resource(&self, _: ResourceAvailable, _: Vec<u8>) -> bool {
            true
        }
        fn publish_diagnostic(&self, _: DiagnosticEvent) {}
        fn failed(&self, _: String) {}
        fn stopped(&self) {
            let mut gate = lock(&self.gate);
            gate.0 = true;
            self.changed.notify_all();
            while !gate.1 {
                gate = self
                    .changed
                    .wait(gate)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
    }
    let sink = Arc::new(PausedStop {
        gate: Mutex::new((false, false)),
        changed: Condvar::new(),
    });
    let host = Arc::new(NativeHost::start(config(), sink.clone()).map_err(|e| format!("{e:?}"))?);
    let first_host = Arc::clone(&host);
    let first = std::thread::spawn(move || first_host.stop_result());
    let entered = sink
        .changed
        .wait_timeout_while(lock(&sink.gate), Duration::from_secs(2), |gate| !gate.0)
        .unwrap_or_else(PoisonError::into_inner);
    let worker_entered = entered.0 .0;
    drop(entered);
    let (begun, starting) = std::sync::mpsc::sync_channel(1);
    let (done, finished) = std::sync::mpsc::sync_channel(1);
    let second_host = Arc::clone(&host);
    let second = std::thread::spawn(move || {
        let _ = begun.send(());
        let _ = done.send(second_host.stop_result());
    });
    let started = starting.recv_timeout(Duration::from_secs(2)).is_ok();
    let returned_before_release = finished.recv_timeout(Duration::from_millis(30)).is_ok();
    lock(&sink.gate).1 = true;
    sink.changed.notify_all();
    first
        .join()
        .map_err(|_| "first stop panicked")?
        .map_err(|e| format!("{e:?}"))?;
    second.join().map_err(|_| "second stop panicked")?;
    assert!(worker_entered && started);
    assert!(!returned_before_release);
    assert_eq!(host.stop_result(), Ok(()));
    Ok(())
}

#[test]
fn snapshot_future_runs_without_a_caller_tokio_runtime() -> Result<(), String> {
    struct WakeThread(std::thread::Thread);
    impl std::task::Wake for WakeThread {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let host = NativeHost::start(config(), Arc::new(Sink)).map_err(|e| format!("{e:?}"))?;
    let mut future = Box::pin(host.snapshot_async());
    let waker = std::task::Waker::from(Arc::new(WakeThread(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let deadline = Instant::now() + Duration::from_secs(2);
    let result = loop {
        if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
            break result;
        }
        if Instant::now() >= deadline {
            return Err("snapshot wake timed out".into());
        }
        std::thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
    };
    assert!(result.is_ok());
    host.stop();
    Ok(())
}

#[test]
fn native_embedding_tracks_prepared_interfaces_in_canonical_snapshot() -> Result<(), String> {
    let embedding = NativeEmbedding {
        prepare_interfaces: Some(Box::new(|client| {
            let attachment = client
                .protocols()
                .attach(TcpClientInterface::new("127.0.0.1:1".to_string()));
            Ok(vec![NativePreparedAttachment::Interface {
                attachment,
                kind: InterfaceKind::TcpClient,
            }])
        })),
        ..NativeEmbedding::default()
    };
    let host = NativeHost::start_with_embedding(config(), Arc::new(Sink), embedding)
        .map_err(|e| format!("{e:?}"))?;
    let snapshot = host
        .snapshot(Some(Duration::from_secs(2)))
        .map_err(|e| format!("{e:?}"))?;
    assert!(snapshot
        .interfaces
        .iter()
        .any(|interface| interface.kind == Some(InterfaceKind::TcpClient)));
    assert!(host.service_client().is_ok());
    host.stop();
    assert!(matches!(
        host.service_client(),
        Err(NativeSubmitError::Stopped)
    ));
    Ok(())
}

#[test]
fn exclusive_native_application_dispatch_requires_callback() {
    let embedding = NativeEmbedding {
        application_events: ApplicationEventDispatch::NativeCallback,
        ..NativeEmbedding::default()
    };
    assert!(matches!(
        NativeHost::start_with_embedding(config(), Arc::new(Sink), embedding),
        Err(NativeStartError::Runtime(_))
    ));
}

#[test]
fn shutdown_retains_persistence_until_blocking_runtime_work_exits() -> Result<(), String> {
    let root = temporary_root("blocking-stop")?;
    let host = Arc::new(
        NativeHost::start(persistent_config(&root), Arc::new(Sink))
            .map_err(|e| format!("{e:?}"))?,
    );
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let blocked = Arc::clone(&gate);
    let (begun, started) = std::sync::mpsc::sync_channel(1);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| e.to_string())?;
    runtime
        .block_on(host.on_preview_runtime(move |_| async move {
            tokio::task::spawn_blocking(move || {
                let _ = begun.send(());
                let (released, changed) = &*blocked;
                let mut released = lock(released);
                while !*released {
                    released = changed
                        .wait(released)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            });
        }))
        .map_err(|e| format!("{e:?}"))?;
    started
        .recv_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    let stopping = Arc::clone(&host);
    let (done, stopped) = std::sync::mpsc::sync_channel(1);
    let stop = std::thread::spawn(move || {
        let _ = done.send(stopping.stop_result());
    });
    // The former five-second runtime shutdown released the owner while this work
    // was still live. Bound only this test wait, never the native owner's lifetime.
    let prematurely_stopped = stopped.recv_timeout(Duration::from_millis(5100)).is_ok();
    let duplicate = NativeHost::start(persistent_config(&root), Arc::new(Sink));
    let (released, changed) = &*gate;
    *lock(released) = true;
    changed.notify_all();
    stop.join().map_err(|_| "stop thread panicked")?;
    assert!(!prematurely_stopped);
    assert!(matches!(
        duplicate,
        Err(NativeStartError::Persistence(
            PersistenceStartError::Unavailable { .. }
        ))
    ));
    host.stop_result().map_err(|e| format!("{e:?}"))?;
    fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn vanished_route_stays_a_typed_no_route_failure() {
    assert_eq!(
        establish_link_failure(SendError::Failed(EstablishLinkFailure::WriteFailed(
            personal_rns::routing::links::establish::WriteEstablishLinkRejection::RouteVanished,
        ))),
        CommandFailure::NoRouteToDestination
    );
}

#[test]
fn native_link_projection_preserves_destination_and_arrival_metadata() -> Result<(), String> {
    let events = NativeSessionEvents::new(PrnsLimits::balanced(), true);
    let stream = events
        .claim_stream(prns_host::ConsumerLane::ApplicationEvents)
        .map_err(|e| format!("{e:?}"))?;
    let destination = DestinationHash::new([6; 16]);
    let link_id = LinkId::new([7; 16]);
    let interface = InterfaceId::new([8; 8]);
    assert!(publish_message(
        &events,
        Message::Delivered(Delivery::Link(
            personal_rns::routing::delivery::LinkDelivery {
                link_id: engine_link(link_id),
                local_destination: Some(engine_destination(destination)),
                plaintext: b"application",
                arrived_at: personal_rns::engine::InstantMillis(4321),
                source_interface: engine_interface(interface),
            }
        ))
    ));
    let event = stream.try_next().map_err(|e| format!("{e:?}"))?;
    let NativeEventValue::Application(ApplicationEvent::LinkDelivery(delivery)) = event.value
    else {
        return Err("wrong event projection".into());
    };
    assert_eq!(delivery.local_destination, Some(destination));
    assert_eq!(delivery.arrived_at_millis, 4321);
    assert_eq!(delivery.link_id, link_id);
    assert_eq!(delivery.source_interface, interface);
    assert_eq!(delivery.plaintext, b"application");
    Ok(())
}

#[test]
fn native_extension_tasks_stay_bounded_after_waiter_cancellation_and_join_on_stop(
) -> Result<(), String> {
    struct ExitSignal(std::sync::mpsc::SyncSender<()>);
    impl Drop for ExitSignal {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let mut configuration = config();
    configuration.limits =
        PrnsLimits::try_new(1, 1, 1024, 1).map_err(|error| format!("{error:?}"))?;
    let host =
        NativeHost::start(configuration, Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
    let (exited_tx, exited_rx) = std::sync::mpsc::sync_channel(1);
    let mut operation = Box::pin(host.on_preview_runtime(move |_| async move {
        let _exit = ExitSignal(exited_tx);
        let _ = started_tx.send(());
        std::future::pending::<()>().await;
    }));
    let mut context = Context::from_waker(std::task::Waker::noop());
    assert!(matches!(
        operation.as_mut().poll(&mut context),
        Poll::Pending
    ));
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .map_err(|error| error.to_string())?;
    drop(operation);
    // Cancellation releases only the foreign waiter, not an admitted operation.
    assert!(matches!(
        exited_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    let mut excess = Box::pin(host.on_preview_runtime(|_| async {}));
    assert!(matches!(
        excess.as_mut().poll(&mut context),
        Poll::Ready(Err(NativeSubmitError::Busy))
    ));
    drop(excess);
    host.stop_result().map_err(|error| format!("{error:?}"))?;
    assert!(
        exited_rx.try_recv().is_ok(),
        "joined stop must have run task drop guards"
    );
    assert_eq!(host.preview_slots.available_permits(), 1);
    Ok(())
}

#[test]
fn joined_stop_preserves_persistence_failure_for_every_waiter() -> Result<(), String> {
    let root = temporary_root("failed-shutdown-flush")?;
    let sink = Arc::new(RecordingSink::new());
    let host = NativeHost::start(persistent_config(&root), sink.clone())
        .map_err(|error| format!("{error:?}"))?;
    if !sink.wait_for(|event| matches!(event, DiagnosticEvent::PersistenceRestored { .. })) {
        return Err("persistence was not restored before failure fixture".into());
    }
    let state = root.join("state");
    fs::remove_dir_all(&state).map_err(|error| error.to_string())?;
    fs::write(&state, b"blocked persistence writes").map_err(|error| error.to_string())?;
    let result = host.stop_result();
    assert_eq!(
        result,
        Err(NativeStopError::NodeFailed(
            personal_rns::runtime::NodeRunError::PersistenceFailed
        ))
    );
    assert_eq!(host.stop_result(), result);
    assert!(sink.diagnostics().iter().any(|event| matches!(
        event,
        DiagnosticEvent::PersistenceFlushFailed {
            cause: PersistenceFlushCause::Shutdown,
            ..
        }
    )));
    fs::remove_dir_all(root).map_err(|error| error.to_string())?;
    Ok(())
}

#[test]
fn remote_control_observations_share_the_application_lane_and_its_byte_bound() -> Result<(), String>
{
    use crate::remote_control_service::NativeRemoteControlEvent;
    use personal_rns::remote_control::RemoteControlPairingEndpoint;
    use personal_rns::units::InstantMillis;
    use prns_host::{ConsumerLane, LifecycleState};
    let limits = PrnsLimits::try_new(1, 2, 4, 1).map_err(|error| format!("{error:?}"))?;
    let events = NativeSessionEvents::new(limits, true);
    let stream = events
        .claim_stream(ConsumerLane::ApplicationEvents)
        .map_err(|error| format!("{error:?}"))?;
    assert!(events
        .claim_stream(ConsumerLane::ApplicationEvents)
        .is_err());
    let make = |data: Vec<u8>| NativeRemoteControlEvent::PairingAvailable {
        endpoint: RemoteControlPairingEndpoint::from_destination_hash(engine_destination(
            DestinationHash::new([1; 16]),
        )),
        observed_at: InstantMillis(20),
        expires_at: InstantMillis(40),
        hops: 2,
        source_interface: personal_rns::interfaces::InterfaceId::from_channel_tag(
            personal_rns::interfaces::InterfaceKind::Pipe,
            b"rc",
        ),
        public_app_data: data,
    };
    events
        .publish_remote_control(make(vec![1, 2]))
        .map_err(|error| format!("{error:?}"))?;
    let rejected = events.publish_remote_control(make(vec![3, 4, 5]));
    assert!(
        matches!(rejected, Err(NativeRemoteControlEvent::PairingAvailable { public_app_data, .. }) if public_app_data == [3,4,5])
    );
    assert!(matches!(
        events.lifecycle().state,
        LifecycleState::Failed(_)
    ));
    assert!(
        matches!(stream.try_next(), Ok(NativeEvent { value: NativeEventValue::RemoteControl(NativeRemoteControlEvent::PairingAvailable { public_app_data, .. }), .. }) if public_app_data == [1,2])
    );
    Ok(())
}

fn remote_control_config(seed: u8) -> crate::remote_control_service::NativeRemoteControlConfig {
    use personal_rns::remote_control::{RemoteControlCapabilities, RemoteControlSelfAnnouncement};
    crate::remote_control_service::NativeRemoteControlConfig {
        controller_identity: IdentityConfig::Existing(prns_host::IdentitySecret::new([seed; 64])),
        target_identity: IdentityConfig::Existing(prns_host::IdentitySecret::new([seed + 1; 64])),
        initial_controller_grants: Vec::new(),
        self_announcement: RemoteControlSelfAnnouncement::Unavailable,
        capabilities: RemoteControlCapabilities::describe_only(),
    }
}

#[test]
fn standalone_remote_control_is_opt_in_and_reuses_host_owned_runtime() -> Result<(), String> {
    use personal_rns::runtime::{
        RemoteControlTargetAccessControl, RemoteControlTargetInventoryControlError,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|error| error.to_string())?;
    let ordinary =
        NativeHost::start(config(), Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
    let unavailable = runtime
        .block_on(ordinary.on_preview_runtime(|handle| async move {
            handle.remote_control_target_inventory().await
        }))
        .map_err(|error| format!("{error:?}"))?;
    assert!(matches!(
        unavailable,
        Err(RemoteControlTargetInventoryControlError::Unavailable)
    ));
    ordinary
        .stop_result()
        .map_err(|error| format!("{error:?}"))?;
    let events = NativeSessionEvents::new(PrnsLimits::balanced(), false);
    let host = NativeHost::start_with_embedding(
        config(),
        Arc::new(events),
        NativeEmbedding {
            remote_control_config: Some(remote_control_config(72)),
            ..NativeEmbedding::default()
        },
    )
    .map_err(|error| format!("{error:?}"))?;
    let inventory = runtime
        .block_on(host.on_preview_runtime(|handle| async move {
            handle.remote_control_target_inventory().await
        }))
        .map_err(|error| format!("{error:?}"))?;
    assert!(
        inventory.is_ok(),
        "explicit service config must enable native operations: {inventory:?}"
    );
    host.stop_result().map_err(|error| format!("{error:?}"))?;
    let mut invalid = remote_control_config(75);
    invalid.target_identity = IdentityConfig::Existing(prns_host::IdentitySecret::new([75; 64]));
    assert!(matches!(
        NativeHost::start_with_embedding(
            config(),
            Arc::new(Sink),
            NativeEmbedding {
                remote_control_config: Some(invalid),
                ..NativeEmbedding::default()
            }
        ),
        Err(NativeStartError::Runtime(_))
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn standalone_pairing_uses_typed_queue_confirmations_and_restores_target_access(
) -> Result<(), String> {
    use crate::remote_control_service::NativeRemoteControlEvent as RcEvent;
    use personal_rns::engine::{
        ApproveRemoteControlControllerPairing, ApproveRemoteControlTargetPairing, EgressTarget,
        OpenRemoteControlPairing,
    };
    use personal_rns::remote_control::{
        RemoteControlPairingAttemptTimeout, RemoteControlPairingExpiresAfter,
        RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
        RemoteControlRequestKind, RemoteControlRequestSet,
    };
    use personal_rns::runtime::{
        InitiateRemoteControlControllerPairing, RemoteControlControllerPairingInitiationControl,
        RemoteControlPairingControl, RemoteControlTargetAccessControl,
    };
    use personal_rns::units::DurationMillis;
    use prns_host::ConsumerLane;

    fn next_rc(
        stream: &NativeEventStream,
        accepts: impl Fn(&RcEvent) -> bool,
    ) -> Result<RcEvent, String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let event = stream
                .next(Some(deadline.saturating_duration_since(Instant::now())))
                .map_err(|error| format!("waiting for remote-control event: {error:?}"))?;
            if let NativeEventValue::RemoteControl(value) = event.value {
                if accepts(&value) {
                    return Ok(value);
                }
            }
        }
    }
    let root = temporary_root("standalone-pairing")?;
    let target_root = root.join("target");
    let controller_root = root.join("controller");
    let target_events = NativeSessionEvents::new(PrnsLimits::balanced(), false);
    let controller_events = NativeSessionEvents::new(PrnsLimits::balanced(), false);
    let target_stream = target_events
        .claim_stream(ConsumerLane::ApplicationEvents)
        .map_err(|error| format!("{error:?}"))?;
    let controller_stream = controller_events
        .claim_stream(ConsumerLane::ApplicationEvents)
        .map_err(|error| format!("{error:?}"))?;
    let target = NativeHost::start_with_embedding(
        persistent_config(&target_root),
        Arc::new(target_events),
        NativeEmbedding {
            remote_control_config: Some(remote_control_config(90)),
            ..NativeEmbedding::default()
        },
    )
    .map_err(|error| format!("{error:?}"))?;
    let controller = NativeHost::start_with_embedding(
        persistent_config(&controller_root),
        Arc::new(controller_events),
        NativeEmbedding {
            remote_control_config: Some(remote_control_config(100)),
            ..NativeEmbedding::default()
        },
    )
    .map_err(|error| format!("{error:?}"))?;
    let (target_wire, controller_wire) =
        std::os::unix::net::UnixStream::pair().map_err(|error| error.to_string())?;
    let (_target_pipe, target_interface) = attach_supplied_wire(&target, "target", target_wire)?;
    let (_controller_pipe, controller_interface) =
        attach_supplied_wire(&controller, "controller", controller_wire)?;
    wait_until_connected(&target, target_interface)?;
    wait_until_connected(&controller, controller_interface)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|error| error.to_string())?;
    let open = OpenRemoteControlPairing {
        target: EgressTarget::AllInterfaces,
        expires_after: RemoteControlPairingExpiresAfter::try_from(DurationMillis(60_000))
            .map_err(|error| format!("{error:?}"))?,
        attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(DurationMillis(30_000))
            .map_err(|error| format!("{error:?}"))?,
        permissions: RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
            RemoteControlRequestKind::Describe,
        ))
        .map_err(|error| format!("{error:?}"))?,
        public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
            b"standalone target".as_slice(),
        )
        .map_err(|error| format!("{error:?}"))?,
    };
    let opened = runtime
        .block_on(target.on_preview_runtime(move |handle| async move {
            handle.open_remote_control_pairing(open).await
        }))
        .map_err(|error| format!("{error:?}"))?
        .map_err(|error| format!("{error:?}"))?;
    let available = next_rc(&controller_stream, |event| {
        matches!(event, RcEvent::PairingAvailable { .. })
    })?;
    let RcEvent::PairingAvailable {
        endpoint,
        expires_at,
        public_app_data,
        ..
    } = available
    else {
        return Err("expected availability".into());
    };
    assert_eq!(public_app_data, b"standalone target");
    let initiate = InitiateRemoteControlControllerPairing {
        endpoint,
        expires_at,
        invitation_code: opened.invitation_code,
    };
    runtime
        .block_on(controller.on_preview_runtime(move |handle| async move {
            handle
                .initiate_remote_control_controller_pairing(initiate)
                .await
        }))
        .map_err(|error| format!("{error:?}"))?
        .map_err(|error| format!("{error:?}"))?;
    let RcEvent::TargetConfirmationRequired {
        confirmation: target_confirmation,
        ..
    } = next_rc(&target_stream, |event| {
        matches!(event, RcEvent::TargetConfirmationRequired { .. })
    })?
    else {
        return Err("expected target confirmation".into());
    };
    let RcEvent::ControllerConfirmationRequired {
        confirmation: controller_confirmation,
        ..
    } = next_rc(&controller_stream, |event| {
        matches!(event, RcEvent::ControllerConfirmationRequired { .. })
    })?
    else {
        return Err("expected controller confirmation".into());
    };
    assert_eq!(
        target_confirmation.attempt_id,
        controller_confirmation.attempt_id
    );
    assert_eq!(
        target_confirmation.confirmation_code,
        controller_confirmation.confirmation_code
    );
    let attempt_id = target_confirmation.attempt_id;
    let target_identity = controller_confirmation.target.identity_hash();
    // Dropping public confirmation views does not discard the engine-owned attempt.
    drop(target_confirmation);
    drop(controller_confirmation);
    runtime
        .block_on(target.on_preview_runtime(move |handle| async move {
            handle
                .approve_remote_control_target_pairing(ApproveRemoteControlTargetPairing {
                    attempt_id,
                })
                .await
        }))
        .map_err(|error| format!("{error:?}"))?
        .map_err(|error| format!("{error:?}"))?;
    runtime
        .block_on(controller.on_preview_runtime(move |handle| async move {
            handle
                .approve_remote_control_controller_pairing(ApproveRemoteControlControllerPairing {
                    attempt_id,
                })
                .await
        }))
        .map_err(|error| format!("{error:?}"))?
        .map_err(|error| format!("{error:?}"))?;
    next_rc(
        &controller_stream,
        |event| matches!(event, RcEvent::ControllerAuthorizationPersisted { attempt_id: actual } if *actual == attempt_id),
    )?;
    target.stop_result().map_err(|error| format!("{error:?}"))?;
    controller
        .stop_result()
        .map_err(|error| format!("{error:?}"))?;
    let restarted = NativeHost::start_with_embedding(
        persistent_config(&controller_root),
        Arc::new(Sink),
        NativeEmbedding {
            remote_control_config: Some(remote_control_config(100)),
            ..NativeEmbedding::default()
        },
    )
    .map_err(|error| format!("{error:?}"))?;
    let inventory = runtime
        .block_on(restarted.on_preview_runtime(|handle| async move {
            handle.remote_control_target_inventory().await
        }))
        .map_err(|error| format!("{error:?}"))?
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(inventory.targets().len(), 1);
    assert_eq!(inventory.targets()[0].identity_hash(), target_identity);
    restarted
        .stop_result()
        .map_err(|error| format!("{error:?}"))?;
    fs::remove_dir_all(root).map_err(|error| error.to_string())?;
    Ok(())
}
