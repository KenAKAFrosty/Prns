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
        || default_policy.gravity.get() != 0
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
