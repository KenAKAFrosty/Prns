#![cfg(feature = "wifi-auto")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroU8;
use std::time::Duration;

use prns_core::interfaces::wifi_auto::{
    DiscoveryEndpoint, DiscoveryServiceName, DiscoverySnapshot, DiscoveryTransport,
    ServiceAdvertisement, TCP_RENDEZVOUS_PORT,
};
use prns_core::interfaces::{InterfaceStatus, ReportsStatus};
use prns_interfaces_tokio::wifi_auto::{
    AutoWifi, DiscoveryParticipation, ServiceDiscovery, ServiceDiscoveryPublisher,
    SnapshotPublication,
};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::runtime::{Fleet, InterfaceSupervisor};
use tokio::net::TcpListener;
use tokio::sync::watch;

const EVENT_DEADLINE: Duration = Duration::from_secs(10);
const TEST_DISCOVERY_CAPACITY: NonZeroU8 = NonZeroU8::new(8).unwrap();

#[derive(Debug)]
enum AwaitError {
    ParticipationTimeout,
    MemberCountTimeout,
    MemberReplacementTimeout,
    DiscoveryLifecycleClosed,
    MemberUpdatesClosed,
}

impl std::fmt::Display for AwaitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParticipationTimeout => {
                formatter.write_str("timed out waiting for participation transition")
            }
            Self::MemberCountTimeout => {
                formatter.write_str("timed out waiting for member-count transition")
            }
            Self::MemberReplacementTimeout => {
                formatter.write_str("timed out waiting for member replacement")
            }
            Self::DiscoveryLifecycleClosed => {
                formatter.write_str("discovery lifecycle closed while awaiting participation")
            }
            Self::MemberUpdatesClosed => {
                formatter.write_str("member updates closed before reaching expected state")
            }
        }
    }
}

impl std::error::Error for AwaitError {}

fn discovery_snapshot(
    service_name: &str,
    ipv4_last_octet: u8,
) -> Result<DiscoverySnapshot, Box<dyn std::error::Error + Send + Sync>> {
    let discovery_service_name =
        DiscoveryServiceName::from_instance(service_name, DiscoveryTransport::Tcp)?;
    let socket_address = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(10, 254, 254, ipv4_last_octet)),
        TCP_RENDEZVOUS_PORT,
    );
    let discovery_endpoint = DiscoveryEndpoint::tcp(socket_address)?;
    let mut service_advertisement = ServiceAdvertisement::new(discovery_service_name);
    let _ = service_advertisement.insert(discovery_endpoint);
    let mut discovery_snapshot = DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY);
    let _ = discovery_snapshot.insert(service_advertisement);
    Ok(discovery_snapshot)
}

async fn await_participation(
    service_discovery_publisher: &mut ServiceDiscoveryPublisher,
    expected_participation: DiscoveryParticipation,
) -> Result<(), AwaitError> {
    match tokio::time::timeout(
        EVENT_DEADLINE,
        service_discovery_publisher.wait_for_participation(expected_participation),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_lifecycle_closed)) => Err(AwaitError::DiscoveryLifecycleClosed),
        Err(_deadline_elapsed) => Err(AwaitError::ParticipationTimeout),
    }
}

async fn await_member_count(
    member_updates: &mut watch::Receiver<Vec<TokioInterfaceStatus>>,
    expected_member_count: usize,
) -> Result<Vec<TokioInterfaceStatus>, AwaitError> {
    match tokio::time::timeout(
        EVENT_DEADLINE,
        member_updates.wait_for(|members| members.len() == expected_member_count),
    )
    .await
    {
        Ok(Ok(members)) => Ok(members.clone()),
        Ok(Err(_member_updates_closed)) => Err(AwaitError::MemberUpdatesClosed),
        Err(_deadline_elapsed) => Err(AwaitError::MemberCountTimeout),
    }
}

async fn await_member_replacement(
    member_updates: &mut watch::Receiver<Vec<TokioInterfaceStatus>>,
    previous_member_id: prns_core::interfaces::InterfaceId,
) -> Result<Vec<TokioInterfaceStatus>, AwaitError> {
    match tokio::time::timeout(
        EVENT_DEADLINE,
        member_updates
            .wait_for(|members| members.len() == 1 && members[0].id() != previous_member_id),
    )
    .await
    {
        Ok(Ok(members)) => Ok(members.clone()),
        Ok(Err(_member_updates_closed)) => Err(AwaitError::MemberUpdatesClosed),
        Err(_deadline_elapsed) => Err(AwaitError::MemberReplacementTimeout),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_discovery_snapshot_and_shared_central_lifecycle_capstone(
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rendezvous_listener = match TcpListener::bind(("0.0.0.0", TCP_RENDEZVOUS_PORT)).await {
        Ok(rendezvous_listener) => rendezvous_listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            let (service_discovery, mut service_discovery_publisher) =
                ServiceDiscovery::channel(TEST_DISCOVERY_CAPACITY);
            let auto_wifi = AutoWifi::new().with_platform_discovery(service_discovery);
            let auto_wifi_status = auto_wifi.status();
            let mut member_updates = auto_wifi_status.subscribe_members();
            let (auto_wifi_fleet, _detached_fleet) = Fleet::detached(auto_wifi_status.id());
            let auto_wifi_task = tokio::spawn(auto_wifi.run(auto_wifi_fleet));
            await_participation(
                &mut service_discovery_publisher,
                DiscoveryParticipation::Satellite,
            )
            .await?;
            let _ = await_member_count(&mut member_updates, 1).await?;
            auto_wifi_status.disable();
            await_participation(
                &mut service_discovery_publisher,
                DiscoveryParticipation::Inactive,
            )
            .await?;
            let _ = await_member_count(&mut member_updates, 0).await?;
            auto_wifi_task.abort();
            let _ = auto_wifi_task.await;
            eprintln!("CAPSTONE: joined the external AutoWifi central as one silent satellite");
            return Ok(());
        }
        Err(error) => {
            return Err(Box::new(error) as Box<dyn std::error::Error + Send + Sync>);
        }
    };
    let (service_discovery, mut service_discovery_publisher) =
        ServiceDiscovery::channel(TEST_DISCOVERY_CAPACITY);
    let auto_wifi = AutoWifi::new()
        .with_platform_discovery(service_discovery)
        .with_rendezvous_listener(rendezvous_listener);
    let auto_wifi_status = auto_wifi.status();
    let mut member_updates = auto_wifi_status.subscribe_members();
    auto_wifi
        .status_view()
        .expect("AutoWifi exposes a status view");
    let (auto_wifi_fleet, _detached_fleet) = Fleet::detached(auto_wifi_status.id());
    let auto_wifi_task = tokio::spawn(auto_wifi.run(auto_wifi_fleet));

    await_participation(
        &mut service_discovery_publisher,
        DiscoveryParticipation::Central,
    )
    .await?;
    let first_snapshot = discovery_snapshot("peer", 2)?;
    assert_eq!(
        service_discovery_publisher.replace_snapshot(first_snapshot),
        SnapshotPublication::Published
    );
    let first_member_id = await_member_count(&mut member_updates, 1).await?[0].id();

    let replacement_snapshot = discovery_snapshot("peer", 3)?;
    assert_eq!(
        service_discovery_publisher.replace_snapshot(replacement_snapshot),
        SnapshotPublication::Published
    );
    let _ = await_member_replacement(&mut member_updates, first_member_id).await?;

    assert_eq!(
        service_discovery_publisher
            .replace_snapshot(DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY)),
        SnapshotPublication::Published
    );
    let _ = await_member_count(&mut member_updates, 0).await?;

    auto_wifi_status.disable();
    await_participation(
        &mut service_discovery_publisher,
        DiscoveryParticipation::Inactive,
    )
    .await?;
    let _ = await_member_count(&mut member_updates, 0).await?;
    let rendezvous_port_guard = TcpListener::bind(("0.0.0.0", TCP_RENDEZVOUS_PORT)).await?;

    auto_wifi_status.enable();
    await_participation(
        &mut service_discovery_publisher,
        DiscoveryParticipation::Satellite,
    )
    .await?;
    let _ = await_member_count(&mut member_updates, 1).await?;
    drop(rendezvous_port_guard);
    await_participation(
        &mut service_discovery_publisher,
        DiscoveryParticipation::Central,
    )
    .await?;
    let _ = await_member_count(&mut member_updates, 0).await?;

    auto_wifi_task.abort();
    let _ = auto_wifi_task.await;
    Ok(())
}
