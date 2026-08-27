use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use prns_core::interfaces::kiss::{
    KissTransmissionControl, ReadyCommandFlowControl, StationIdInterval, StationIdWireFormat,
};
use prns_core::interfaces::kiss_framing;
use prns_core::interfaces::rnode::multi::{RadioConfigInput, VPort};
use prns_core::interfaces::rnode::policy;
use prns_core::interfaces::{BitrateBps, ConnectionState, InterfaceStatus};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::throughput::ThroughputLedger;

use super::bring_up::bring_up;
use super::member::{InboundFrame, LiveMember, MemberMeters, OutboundFrame};
use super::wire::WireCycle;
use super::*;

fn member(vport: u8) -> RNodeMultiMemberSettings {
    let radio = multi::RadioConfig::new(RadioConfigInput {
        frequency_hz: if vport == 0 {
            868_000_000
        } else {
            2_400_000_000
        },
        bandwidth_hz: 125_000,
        tx_power_dbm: 7,
        spreading_factor: 8,
        coding_rate: 5,
        airtime_limit_short_centi_percent: None,
        airtime_limit_long_centi_percent: None,
    })
    .expect("valid radio");
    RNodeMultiMemberSettings::new(
        format!("Dual[{vport}]"),
        VPort::new(vport).expect("valid vport"),
        radio,
        ReadyCommandFlowControl::Disabled,
        policy::policy_for_bitrate(BitrateBps::guess(u64::from(radio.nominal_bitrate_bps()))),
        RNodeMultiAccess::Open,
        b"test-device",
    )
}

async fn read_commands<R: AsyncRead + Unpin>(wire: &mut R, wanted: usize) -> Vec<(u8, Vec<u8>)> {
    let mut decoder = protocol::CommandDecoder::new();
    let mut read = [0u8; 512];
    let mut commands = Vec::new();
    while commands.len() < wanted {
        let read_count = wire.read(&mut read).await.expect("device reads host data");
        assert_ne!(read_count, 0);
        let mut offset = 0;
        while offset < read_count {
            if let Some((command, payload)) = decoder
                .feed_slice_next(&read[..read_count], &mut offset)
                .expect("valid host framing")
            {
                commands.push((command, payload.to_vec()));
            }
        }
    }
    commands
}

async fn write_command<W: AsyncWrite + Unpin>(wire: &mut W, command: u8, payload: &[u8]) {
    let mut frame = [0u8; 128];
    let framed = kiss_framing::encode_with_command(command, payload, &mut frame)
        .expect("device command fits");
    wire.write_all(&frame[..framed])
        .await
        .expect("device writes host data");
}

async fn answer_radio<RW: AsyncRead + AsyncWrite + Unpin>(
    wire: &mut RW,
    member: &RNodeMultiMemberSettings,
) {
    let commands = read_commands(wire, 12).await;
    for selected in commands.iter().step_by(2) {
        assert_eq!(
            selected,
            &(multi::CMD_SELECT_INTERFACE, vec![member.vport.get()])
        );
    }
    write_command(wire, multi::CMD_SELECT_INTERFACE, &[member.vport.get()]).await;
    write_command(
        wire,
        protocol::CMD_FREQUENCY,
        &member.radio.frequency().hz().to_be_bytes(),
    )
    .await;
    write_command(
        wire,
        protocol::CMD_BANDWIDTH,
        &member.radio.bandwidth_hz().to_be_bytes(),
    )
    .await;
    write_command(
        wire,
        protocol::CMD_TXPOWER,
        &[member.radio.tx_power_dbm() as u8],
    )
    .await;
    write_command(wire, protocol::CMD_SF, &[member.radio.spreading_factor()]).await;
    write_command(wire, protocol::CMD_CR, &[member.radio.coding_rate()]).await;
    write_command(wire, protocol::CMD_RADIO_STATE, &[protocol::RADIO_STATE_ON]).await;
}

async fn answer_bring_up<RW: AsyncRead + AsyncWrite + Unpin>(
    wire: &mut RW,
    members: &[RNodeMultiMemberSettings],
) {
    let detect = read_commands(wire, 5).await;
    assert_eq!(
        detect[0],
        (protocol::CMD_DETECT, vec![protocol::DETECT_REQ])
    );
    assert_eq!(detect[4].0, multi::CMD_INTERFACES);
    write_command(wire, protocol::CMD_DETECT, &[protocol::DETECT_RESP]).await;
    write_command(wire, protocol::CMD_FW_VERSION, &[1, 80]).await;
    write_command(wire, protocol::CMD_PLATFORM, &[0x80]).await;
    write_command(wire, multi::CMD_INTERFACES, &[0, 0x10, 1, 0x20]).await;
    for member in members {
        answer_radio(wire, member).await;
    }
}

fn wire_cycle(
    members: &[RNodeMultiMemberSettings],
    station_identification: Option<StationIdentification>,
) -> (WireCycle, Vec<mpsc::UnboundedReceiver<InboundFrame>>) {
    let (_outbound_tx, outbound) = mpsc::unbounded_channel();
    let started = tokio::time::Instant::now();
    let mut inbound_receivers = Vec::new();
    let members: Vec<LiveMember> = members
        .iter()
        .map(|settings| {
            let (inbound, inbound_rx) = mpsc::unbounded_channel();
            inbound_receivers.push(inbound_rx);
            LiveMember {
                vport: settings.vport,
                radio: settings.radio,
                inbound,
                control: KissTransmissionControl::new(
                    settings.flow_control,
                    station_identification.clone(),
                ),
                meters: MemberMeters {
                    status: TokioInterfaceStatus::new_unaccounted(
                        settings.id(),
                        ConnectionState::Connected,
                    ),
                    airtime: AirtimeLedger::new(),
                    throughput: ThroughputLedger::new(),
                    started,
                    bitrate: settings.policy.bitrate,
                },
            }
        })
        .collect();
    let live = multi::live::LiveProtocol::new(
        members.iter().map(|member| multi::ConfiguredRadio {
            vport: member.vport,
            radio: member.radio,
        }),
        None,
    );
    (
        WireCycle {
            members,
            outbound,
            live,
            started,
        },
        inbound_receivers,
    )
}

#[test]
fn member_set_cannot_be_empty_or_reuse_a_vport() {
    assert!(matches!(
        RNodeMultiMembers::new(Vec::new()),
        Err(RNodeMultiMembersError::Empty)
    ));
    assert!(matches!(
        RNodeMultiMembers::new(vec![member(0), member(0)]),
        Err(RNodeMultiMembersError::DuplicateVPort(vport)) if vport == VPort::ZERO
    ));
    assert!(RNodeMultiMembers::new(vec![member(0), member(1)]).is_ok());
}

#[test]
fn child_ids_are_stable_per_device_and_distinct_per_vport() {
    let low = member(0);
    let high = member(1);
    assert_eq!(low.id(), member(0).id());
    assert_ne!(low.id(), high.id());
}

#[tokio::test]
async fn bring_up_detects_inventory_and_validates_each_selected_radio() {
    let members = vec![member(0), member(1)];
    let expected = members.clone();
    let (mut host, mut device) = tokio::io::duplex(16_384);
    let task = tokio::spawn(async move {
        let mut decoder = protocol::CommandDecoder::new();
        let mut read = [0u8; protocol::READ_BUF_LEN];
        bring_up(
            &mut host,
            &members,
            RNodeMultiConfigureDelay::new(Duration::ZERO),
            &mut decoder,
            &mut read,
        )
        .await
    });

    answer_bring_up(&mut device, &expected).await;
    let platform = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("bring-up completes")
        .expect("bring-up task joins")
        .expect("both radios validate");
    assert_eq!(platform, Some(multi::DevicePlatform::Esp32));
}

#[tokio::test]
async fn bring_up_names_the_missing_vport_and_reported_inventory() {
    let members = vec![member(0), member(1)];
    let (mut host, mut device) = tokio::io::duplex(4096);
    let task = tokio::spawn(async move {
        let mut decoder = protocol::CommandDecoder::new();
        let mut read = [0u8; protocol::READ_BUF_LEN];
        bring_up(
            &mut host,
            &members,
            RNodeMultiConfigureDelay::new(Duration::ZERO),
            &mut decoder,
            &mut read,
        )
        .await
    });

    let _ = read_commands(&mut device, 5).await;
    write_command(&mut device, protocol::CMD_DETECT, &[protocol::DETECT_RESP]).await;
    write_command(&mut device, protocol::CMD_FW_VERSION, &[1, 80]).await;
    write_command(&mut device, multi::CMD_INTERFACES, &[0, 0x10]).await;
    let error = task
        .await
        .expect("bring-up task joins")
        .expect_err("missing vport fails bring-up");
    assert_eq!(
        error.to_string(),
        "RNodeMulti vport 1 is not present; the device reported 1 radio(s)"
    );
}

#[tokio::test]
async fn selected_vport_demultiplexes_inbound_packets_and_metrics() {
    let settings = vec![member(0), member(1)];
    let (mut wire, mut inbound) = wire_cycle(&settings, None);
    let (mut host, _device) = tokio::io::duplex(1024);
    let mut decoder = protocol::CommandDecoder::new();

    let low_frame = multi::data_frame(VPort::ZERO, b"low").expect("low frame");
    wire.apply_read(&low_frame, &mut decoder, &mut host)
        .await
        .expect("low frame applies");
    assert_eq!(inbound[0].recv().await.expect("low packet").payload, b"low");
    assert!(inbound[1].try_recv().is_err());
    assert!(wire.members[0].meters.status.rx_bytes() > 0);
    assert_eq!(wire.members[1].meters.status.rx_bytes(), 0);

    let high_frame =
        multi::data_frame(VPort::new(1).expect("vport one"), b"high").expect("high frame");
    wire.apply_read(&high_frame, &mut decoder, &mut host)
        .await
        .expect("high frame applies");
    assert_eq!(
        inbound[1].recv().await.expect("high packet").payload,
        b"high"
    );
    assert!(wire.members[1].meters.status.rx_bytes() > 0);
}

#[tokio::test]
async fn hardware_error_frames_end_the_live_wire_cycle_with_actionable_failures() {
    let cases = [
        (
            protocol::ERROR_INIT_RADIO,
            "RNodeMulti radio initialisation failure",
        ),
        (
            protocol::ERROR_TX_FAILED,
            "RNodeMulti hardware transmit failure",
        ),
        (protocol::ERROR_EEPROM_LOCKED, "RNodeMulti EEPROM is locked"),
        (0xff, "RNodeMulti unknown hardware failure"),
    ];
    for (code, expected) in cases {
        let (mut wire, _) = wire_cycle(&[member(0)], None);
        let (mut host, _device) = tokio::io::duplex(1024);
        let mut decoder = protocol::CommandDecoder::new();
        let mut frame = [0u8; 16];
        let frame_len = kiss_framing::encode_with_command(protocol::CMD_ERROR, &[code], &mut frame)
            .expect("error frame fits");
        let error = wire
            .apply_read(&frame[..frame_len], &mut decoder, &mut host)
            .await
            .expect_err("hardware errors end the connection");
        assert_eq!(error.to_string(), expected);
    }
}

#[tokio::test]
async fn only_an_online_esp32_reset_ends_the_live_wire_cycle() {
    let (mut wire, _) = wire_cycle(&[member(0)], None);
    let (mut host, _device) = tokio::io::duplex(1024);
    let mut decoder = protocol::CommandDecoder::new();
    let mut frame = [0u8; 16];
    let platform_len =
        kiss_framing::encode_with_command(protocol::CMD_PLATFORM, &[0x80], &mut frame)
            .expect("platform frame fits");
    wire.apply_read(&frame[..platform_len], &mut decoder, &mut host)
        .await
        .expect("late platform report applies");
    let frame_len =
        kiss_framing::encode_with_command(protocol::CMD_RESET, &[protocol::RESET_RESP], &mut frame)
            .expect("reset frame fits");
    let error = wire
        .apply_read(&frame[..frame_len], &mut decoder, &mut host)
        .await
        .expect_err("ESP32 reset ends the connection");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert_eq!(error.to_string(), "RNodeMulti ESP32 reset while online");

    let (mut wire, _) = wire_cycle(&[member(0)], None);
    let mut decoder = protocol::CommandDecoder::new();
    let platform_len =
        kiss_framing::encode_with_command(protocol::CMD_PLATFORM, &[0x70], &mut frame)
            .expect("platform frame fits");
    wire.apply_read(&frame[..platform_len], &mut decoder, &mut host)
        .await
        .expect("late platform report applies");
    let frame_len =
        kiss_framing::encode_with_command(protocol::CMD_RESET, &[protocol::RESET_RESP], &mut frame)
            .expect("reset frame fits");
    wire.apply_read(&frame[..frame_len], &mut decoder, &mut host)
        .await
        .expect("the reference only reinitialises an ESP32 reset");
}

#[tokio::test]
async fn ready_releases_each_radios_own_queue_without_blocking_the_other_radio() {
    let mut low = member(0);
    low.flow_control = ReadyCommandFlowControl::WaitForReady;
    let settings = vec![low, member(1)];
    let (mut wire, _inbound) = wire_cycle(&settings, None);
    let (mut host, mut device) = tokio::io::duplex(4096);

    wire.accept_outbound(
        OutboundFrame {
            vport: VPort::ZERO,
            payload: b"low-one".to_vec(),
        },
        &mut host,
    )
    .await
    .expect("first low packet writes");
    assert_eq!(
        read_commands(&mut device, 2).await,
        vec![
            (multi::CMD_SELECT_INTERFACE, vec![0]),
            (protocol::CMD_DATA, b"low-one".to_vec())
        ]
    );

    wire.accept_outbound(
        OutboundFrame {
            vport: VPort::ZERO,
            payload: b"low-two".to_vec(),
        },
        &mut host,
    )
    .await
    .expect("second low packet queues");
    wire.accept_outbound(
        OutboundFrame {
            vport: VPort::new(1).expect("vport one"),
            payload: b"high".to_vec(),
        },
        &mut host,
    )
    .await
    .expect("high packet writes independently");
    assert_eq!(
        read_commands(&mut device, 2).await,
        vec![
            (multi::CMD_SELECT_INTERFACE, vec![1]),
            (protocol::CMD_DATA, b"high".to_vec())
        ]
    );

    wire.release_ready(&mut host)
        .await
        .expect("READY releases low queue");
    assert_eq!(
        read_commands(&mut device, 2).await,
        vec![
            (multi::CMD_SELECT_INTERFACE, vec![0]),
            (protocol::CMD_DATA, b"low-two".to_vec())
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn station_identification_armed_by_one_radio_broadcasts_on_every_radio() {
    let station_identification = StationIdentification::new(
        b"N0CALL",
        StationIdInterval::new(Duration::from_millis(100)),
        StationIdWireFormat::Exact,
    )
    .expect("valid station identification");
    let settings = vec![member(0), member(1)];
    let (mut wire, _inbound) = wire_cycle(&settings, Some(station_identification));
    let (mut host, mut device) = tokio::io::duplex(4096);

    wire.accept_outbound(
        OutboundFrame {
            vport: VPort::ZERO,
            payload: b"traffic".to_vec(),
        },
        &mut host,
    )
    .await
    .expect("traffic writes");
    let _ = read_commands(&mut device, 2).await;
    tokio::time::advance(Duration::from_millis(100)).await;
    wire.emit_station_identification(&mut host)
        .await
        .expect("station identification writes");
    assert_eq!(
        read_commands(&mut device, 4).await,
        vec![
            (multi::CMD_SELECT_INTERFACE, vec![0]),
            (protocol::CMD_DATA, b"N0CALL".to_vec()),
            (multi::CMD_SELECT_INTERFACE, vec![1]),
            (protocol::CMD_DATA, b"N0CALL".to_vec()),
        ]
    );
}

#[tokio::test]
async fn a_serial_drop_removes_and_recreates_every_logical_radio_together() {
    use prns_runtime::runtime::{
        ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
    };
    use prns_runtime::storage::GrowableHeap;

    let settings = vec![member(0), member(1)];
    let expected = settings.clone();
    let expected_ids = settings
        .iter()
        .map(RNodeMultiMemberSettings::id)
        .collect::<HashSet<_>>();
    let members = RNodeMultiMembers::new(settings).expect("valid members");
    let (first_host, mut first_device) = tokio::io::duplex(16_384);
    let (second_host, mut second_device) = tokio::io::duplex(16_384);
    let mut host_wires = VecDeque::from([first_host, second_host]);
    let interface = RNodeMultiInterface::new(
        "Dual",
        "test-device",
        move || {
            let opened = host_wires.pop_front();
            async move { opened.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        },
        RNodeMultiSettings {
            reconnect_policy: ReconnectPolicy::STANDARD,
            reset_delay: RNodeResetDelay::new(Duration::ZERO),
            configure_delay: RNodeMultiConfigureDelay::new(Duration::ZERO),
            station_identification: None,
            members,
        },
    );
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: prns_runtime::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    });
    let handle = node.handle();
    let registered = interface.register(&handle);
    assert_eq!(interface_ids(&handle), expected_ids);
    assert!(handle
        .interfaces()
        .iter()
        .all(|snapshot| snapshot.connection == ConnectionState::Initializing));
    let supervisor = tokio::spawn(registered.run());

    tokio::select! {
        result = node.run() => panic!("node stays running: {result:?}"),
        () = async {
            wait_for_interface_count(&handle, 2).await;
            assert!(handle.interfaces().iter().all(|snapshot| {
                snapshot.connection == ConnectionState::Initializing
            }));
            answer_bring_up(&mut first_device, &expected).await;
            wait_for_connection(&handle, ConnectionState::Connected).await;
            assert_eq!(interface_ids(&handle), expected_ids);
            drop(first_device);
            wait_for_interface_count(&handle, 0).await;
            wait_for_interface_count(&handle, 2).await;
            answer_bring_up(&mut second_device, &expected).await;
            wait_for_connection(&handle, ConnectionState::Connected).await;
            assert_eq!(interface_ids(&handle), expected_ids);
        } => {}
    }
    supervisor.abort();
}

async fn wait_for_interface_count(handle: &PrnsNodeHandle, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while handle.interfaces().len() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("interface count settles");
}

async fn wait_for_connection(handle: &PrnsNodeHandle, expected: ConnectionState) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while handle
            .interfaces()
            .iter()
            .any(|snapshot| snapshot.connection != expected)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("interface connection states settle");
}

fn interface_ids(handle: &PrnsNodeHandle) -> HashSet<InterfaceId> {
    handle
        .interfaces()
        .into_iter()
        .map(|snapshot| snapshot.id)
        .collect()
}
