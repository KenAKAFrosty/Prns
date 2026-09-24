use std::process::Stdio;
use std::time::Duration;

use personal_rns::engine::RequestPathFailure;
use personal_rns::interfaces::ConnectionState;
use personal_rns::prelude::*;
use personal_rns::runtime::RequestPathError;
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpServer;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use personal_hopspot_headless::tcp::TcpMode;

async fn cold_path(mode: TcpMode) {
    let directory = tempfile::tempdir().unwrap();
    let mut target = Command::new(env!("CARGO_BIN_EXE_personal-hopspot-headless"))
        .arg("--state-dir")
        .arg(directory.path())
        .args(["--listen", "127.0.0.1:0", "--run-for", "30"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut output = BufReader::new(target.stdout.take().unwrap());
    let mut ready = String::new();
    output.read_line(&mut ready).await.unwrap();
    let address = ready
        .strip_prefix("hopspot_ready listen=")
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    let mut destination = [0; 16];
    hex::decode_to_slice(
        ready.split("node_page=").nth(1).unwrap().trim(),
        &mut destination,
    )
    .unwrap();
    let destination = DestinationHash::new(destination);
    let server = TcpServer::bind_with_policy("127.0.0.1:0", mode.policy())
        .await
        .unwrap();
    let gateway_address = server.local_addr().unwrap();
    let gateway = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(Zeroizing::new([23; 64])),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        remote_control: RemoteControlService::Unavailable.into(),
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_, _: &()| {},
    });
    let upstream = TcpClientInterface::new(address.to_owned());
    let upstream_status = upstream.status();
    gateway.handle().add_interface(upstream);
    gateway.handle().supervise(server);
    let controller = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        remote_control: RemoteControlService::Unavailable.into(),
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_, _: &()| {},
    });
    let ingress = TcpClientInterface::new(gateway_address.to_string());
    let ingress_status = ingress.status();
    let handle = controller.handle();
    handle.add_interface(ingress);
    let exercise = async {
        while upstream_status.connection() != ConnectionState::Connected
            || ingress_status.connection() != ConnectionState::Connected
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // The target has no announce policy; the relay cannot already know its route.
        let result = handle.request_path(destination).await;
        match mode {
            TcpMode::PointToPoint => assert!(matches!(
                result,
                Err(RequestPathError::Failed(RequestPathFailure::Timeout))
            )),
            TcpMode::Gateway => {
                result.unwrap();
                handle.establish_link(destination).await.unwrap();
            }
        }
    };
    tokio::select! {
        () = exercise => {},
        result = gateway.run() => panic!("gateway stopped: {result:?}"),
        result = controller.run() => panic!("controller stopped: {result:?}"),
    }
    target.kill().await.unwrap();
    target.wait().await.unwrap();
}

#[tokio::test]
async fn cold_discovery_through_a_tcp_ingress_requires_gateway_mode() {
    tokio::time::timeout(Duration::from_secs(25), async {
        cold_path(TcpMode::PointToPoint).await;
        cold_path(TcpMode::Gateway).await;
    })
    .await
    .expect("TCP gateway qualification timed out");
}
