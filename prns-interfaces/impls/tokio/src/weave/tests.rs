#![allow(clippy::expect_used)]

use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use super::status::WeaveInterfaceStatus;
use super::supervisor::WeaveInterface;
use crate::reconnect::ReconnectPolicy;
use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use prns_core::interfaces::weave::{core, wdcl};
use prns_core::interfaces::{ConnectionState, InterfaceId, InterfaceStatus};
use prns_runtime::runtime::{Fleet, InterfaceSupervisor};

#[test]
fn status_distinguishes_initial_start_retry_and_live_device() {
    let status = WeaveInterfaceStatus::new(InterfaceId::new([0x77; 8]));
    assert_eq!(status.connection(), ConnectionState::Initializing);
    status.complete_initial_attempt();
    assert_eq!(status.connection(), ConnectionState::Reconnecting);
    status.mark_connected();
    assert_eq!(status.connection(), ConnectionState::Connected);
    status.disable();
    assert_eq!(status.connection(), ConnectionState::Disabled);
}

async fn read_frame<Stream: AsyncRead + Unpin>(
    stream: &mut Stream,
    decoder: &mut RnsSerialDecoder<{ core::WDCL_MAX_CHUNK }>,
) -> Vec<u8> {
    let mut read = [0u8; 1_500];
    loop {
        let read_len = stream
            .read(&mut read)
            .await
            .expect("the fake device reads a host frame");
        let mut offset = 0;
        while offset < read_len {
            if let Some(frame) = decoder
                .feed_slice_next(&read[..read_len], &mut offset)
                .expect("the host frame fits the WDCL maximum")
            {
                return frame.to_vec();
            }
        }
    }
}

fn event_frame(host_switch: wdcl::SwitchId, event: u16, data: &[u8]) -> Vec<u8> {
    let mut raw = vec![0u8; wdcl::SwitchId::LEN + 1 + 1 + 8 + data.len()];
    raw[..wdcl::SwitchId::LEN].copy_from_slice(host_switch.as_bytes());
    raw[wdcl::SwitchId::LEN] = wdcl::TYPE_LOG;
    raw[12..14].copy_from_slice(&event.to_be_bytes());
    raw[14..].copy_from_slice(data);
    let mut framed = vec![0u8; rns_serial_framing::max_encoded_len(raw.len())];
    let written = rns_serial_framing::encode(&raw, &mut framed)
        .expect("the fake event frame output is exactly sized");
    framed.truncate(written);
    framed
}

#[tokio::test]
async fn fake_device_completes_handshake_and_populates_endpoint_members() {
    let host_identity = wdcl::WeaveHostIdentity::from_signing_secret([0x11; 32]);
    let host_switch = host_identity.switch_id();
    let device_identity = wdcl::WeaveHostIdentity::from_signing_secret([0x22; 32]);
    let device_switch = device_identity.switch_id();
    let (host_wire, mut device_wire) = tokio::io::duplex(core::FRAMED_LEN);
    let mut host_wire = Some(host_wire);
    let open = move || {
        let opened = host_wire.take();
        async move { opened.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
    };
    let interface = WeaveInterface::with_identity(
        open,
        ReconnectPolicy::STANDARD,
        core::configured_policy(Default::default()),
        b"fake-weave",
        host_identity,
    );
    let status = interface.status();
    let (fleet, _tail) = Fleet::detached(interface.id());
    let task = tokio::spawn(interface.run(fleet));

    let mut decoder = RnsSerialDecoder::<{ core::WDCL_MAX_CHUNK }>::new();
    let discovery = read_frame(&mut device_wire, &mut decoder).await;
    assert_eq!(&discovery[..4], wdcl::BROADCAST_SWITCH.as_bytes());
    assert_eq!(discovery[4], wdcl::TYPE_DISCOVER);
    assert_eq!(&discovery[5..], host_switch.as_bytes());

    let mut response = [0u8; 256];
    let response_len =
        wdcl::encode_discovery_response(&device_identity, host_switch, &mut response)
            .expect("the discovery response fits");
    device_wire
        .write_all(&response[..response_len])
        .await
        .expect("the fake device writes its discovery response");

    let handshake = read_frame(&mut device_wire, &mut decoder).await;
    assert_eq!(&handshake[..4], device_switch.as_bytes());
    assert_eq!(handshake[4], wdcl::TYPE_CONNECT);

    device_wire
        .write_all(&event_frame(host_switch, wdcl::EVENT_WDCL_CONNECTION, &[]))
        .await
        .expect("the fake device reports the connection");
    let host_endpoint = wdcl::EndpointId::new([0x44; 8]);
    device_wire
        .write_all(&event_frame(
            host_switch,
            wdcl::EVENT_WDCL_HOST_ENDPOINT,
            host_endpoint.as_bytes(),
        ))
        .await
        .expect("the fake device reports the host endpoint");
    let first_peer = wdcl::EndpointId::new([0x55; 8]);
    device_wire
        .write_all(&event_frame(
            host_switch,
            wdcl::EVENT_WEAVE_ENDPOINT_ALIVE,
            first_peer.as_bytes(),
        ))
        .await
        .expect("the fake device reports a peer");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if status.connection() == ConnectionState::Connected
                && status.remote_switch() == Some(device_switch)
                && status.host_endpoint() == Some(host_endpoint)
                && status.member_vitals().len() == 1
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the supervisor reflects the authenticated device and peer");

    device_wire
        .write_all(&event_frame(
            host_switch,
            wdcl::EVENT_WEAVE_ENDPOINT_ALIVE,
            first_peer.as_bytes(),
        ))
        .await
        .expect("the fake device refreshes the existing peer");
    tokio::task::yield_now().await;
    assert_eq!(status.member_vitals().len(), 1);

    task.abort();
}
