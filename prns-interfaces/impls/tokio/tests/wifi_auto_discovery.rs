#![cfg(feature = "wifi-auto")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroU8;
use std::time::Duration;

use prns_core::interfaces::wifi_auto::{
    DiscoveryEndpoint, DiscoveryServiceName, DiscoverySnapshot, ServiceAdvertisement,
    TCP_RENDEZVOUS_PORT,
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
    Timeout(&'static str),
    PublisherClosed(&'static str),
}

impl std::fmt::Display for AwaitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout(state) => write!(formatter, "timed out waiting for {state}"),
            Self::PublisherClosed(state) => {
                write!(formatter, "publisher closed while waiting for {state}")
            }
        }
    }
}

impl std::error::Error for AwaitError {}

fn snapshot(
    name: &str,
    last_octet: u8,
) -> Result<DiscoverySnapshot, Box<dyn std::error::Error + Send + Sync>> {
    let service = DiscoveryServiceName::new(name)?;
    let address = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(10, 254, 254, last_octet)),
        TCP_RENDEZVOUS_PORT,
    );
    let endpoint = DiscoveryEndpoint::new(address)?;
    let mut advertisement = ServiceAdvertisement::new(service);
    let _ = advertisement.insert(endpoint);
    let mut snapshot = DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY);
    let _ = snapshot.insert(advertisement);
    Ok(snapshot)
}

async fn await_participation(
    publisher: &mut ServiceDiscoveryPublisher,
    expected: DiscoveryParticipation,
) -> Result<(), AwaitError> {
    match tokio::time::timeout(EVENT_DEADLINE, publisher.wait_for_participation(expected)).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(AwaitError::PublisherClosed("participation transition")),
        Err(_) => Err(AwaitError::Timeout("participation transition")),
    }
}

async fn await_member_count(
    updates: &mut watch::Receiver<Vec<TokioInterfaceStatus>>,
    expected: usize,
) -> Result<Vec<TokioInterfaceStatus>, AwaitError> {
    match tokio::time::timeout(
        EVENT_DEADLINE,
        updates.wait_for(|members| members.len() == expected),
    )
    .await
    {
        Ok(Ok(members)) => Ok(members.clone()),
        Ok(Err(_)) => Err(AwaitError::PublisherClosed("member-set transition")),
        Err(_) => Err(AwaitError::Timeout("member-set transition")),
    }
}

async fn await_member_replacement(
    updates: &mut watch::Receiver<Vec<TokioInterfaceStatus>>,
    previous: prns_core::interfaces::InterfaceId,
) -> Result<Vec<TokioInterfaceStatus>, AwaitError> {
    match tokio::time::timeout(
        EVENT_DEADLINE,
        updates.wait_for(|members| members.len() == 1 && members[0].id() != previous),
    )
    .await
    {
        Ok(Ok(members)) => Ok(members.clone()),
        Ok(Err(_)) => Err(AwaitError::PublisherClosed("member replacement")),
        Err(_) => Err(AwaitError::Timeout("member replacement")),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_discovery_snapshot_and_shared_core_lifecycle_capstone(
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = match TcpListener::bind(("0.0.0.0", TCP_RENDEZVOUS_PORT)).await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            let (discovery, mut publisher) = ServiceDiscovery::channel(TEST_DISCOVERY_CAPACITY);
            let wifi = AutoWifi::new().with_platform_discovery(discovery);
            let status = wifi.status();
            let mut member_updates = status.subscribe_members();
            let (fleet, _tail) = Fleet::detached(status.id());
            let task = tokio::spawn(wifi.run(fleet));
            await_participation(&mut publisher, DiscoveryParticipation::Satellite).await?;
            let _ = await_member_count(&mut member_updates, 1).await?;
            status.disable();
            await_participation(&mut publisher, DiscoveryParticipation::Inactive).await?;
            let _ = await_member_count(&mut member_updates, 0).await?;
            task.abort();
            let _ = task.await;
            eprintln!("CAPSTONE: joined the external AutoWifi core as one silent satellite");
            return Ok(());
        }
        Err(error) => {
            return Err(Box::new(error) as Box<dyn std::error::Error + Send + Sync>);
        }
    };
    let (discovery, mut publisher) = ServiceDiscovery::channel(TEST_DISCOVERY_CAPACITY);
    let wifi = AutoWifi::new()
        .with_platform_discovery(discovery)
        .with_rendezvous_listener(listener);
    let status = wifi.status();
    let mut member_updates = status.subscribe_members();
    let status_view = wifi.status_view();
    assert!(status_view.is_some());
    let (fleet, _tail) = Fleet::detached(status.id());
    let task = tokio::spawn(wifi.run(fleet));

    await_participation(&mut publisher, DiscoveryParticipation::Core).await?;
    let first_snapshot = snapshot("peer", 2)?;
    assert_eq!(
        publisher.replace_snapshot(first_snapshot),
        SnapshotPublication::Published
    );
    let first = await_member_count(&mut member_updates, 1).await?[0].id();

    let second_snapshot = snapshot("peer", 3)?;
    assert_eq!(
        publisher.replace_snapshot(second_snapshot),
        SnapshotPublication::Published
    );
    let _ = await_member_replacement(&mut member_updates, first).await?;

    assert_eq!(
        publisher.replace_snapshot(DiscoverySnapshot::new(TEST_DISCOVERY_CAPACITY)),
        SnapshotPublication::Published
    );
    let _ = await_member_count(&mut member_updates, 0).await?;

    status.disable();
    await_participation(&mut publisher, DiscoveryParticipation::Inactive).await?;
    let _ = await_member_count(&mut member_updates, 0).await?;
    let probe = TcpListener::bind(("0.0.0.0", TCP_RENDEZVOUS_PORT)).await?;

    status.enable();
    await_participation(&mut publisher, DiscoveryParticipation::Satellite).await?;
    let _ = await_member_count(&mut member_updates, 1).await?;
    drop(probe);
    await_participation(&mut publisher, DiscoveryParticipation::Core).await?;
    let _ = await_member_count(&mut member_updates, 0).await?;

    task.abort();
    let _ = task.await;
    Ok(())
}
