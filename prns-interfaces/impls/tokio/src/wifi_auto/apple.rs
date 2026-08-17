use std::time::Duration;

use prns_ffi::mdns::macos::{AppleServiceDiscoveryBackend, MdnsError, DISCOVERY_CAPACITY};

use super::{
    DiscoveryLifecycleError, DiscoveryParticipation, ServiceDiscovery, ServiceDiscoveryPublisher,
    SnapshotPublication,
};

const RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Starts Apple Network Services behind a bounded AutoWifi discovery channel.
///
/// The native run loop exists only while the associated AutoWifi runtime is
/// the local [`DiscoveryParticipation::Central`].
pub fn apple_service_discovery() -> ServiceDiscovery {
    let (service_discovery, service_discovery_publisher) =
        ServiceDiscovery::channel(DISCOVERY_CAPACITY);
    tokio::spawn(run_service_discovery(service_discovery_publisher));
    service_discovery
}

async fn run_service_discovery(
    mut service_discovery_publisher: ServiceDiscoveryPublisher,
) -> AppleDiscoveryExit {
    loop {
        if service_discovery_publisher.participation() != DiscoveryParticipation::Central {
            service_discovery_publisher.clear_snapshot();
            match service_discovery_publisher
                .wait_for_participation(DiscoveryParticipation::Central)
                .await
            {
                Ok(()) => {}
                Err(DiscoveryLifecycleError::Closed) => {
                    return AppleDiscoveryExit::RuntimeDropped;
                }
            }
        }

        match run_central_session(&mut service_discovery_publisher).await {
            Ok(
                CentralDiscoverySessionEnd::BecameInactive
                | CentralDiscoverySessionEnd::BecameSatellite,
            ) => {
                service_discovery_publisher.clear_snapshot();
                continue;
            }
            Ok(CentralDiscoverySessionEnd::RuntimeDropped) => {
                service_discovery_publisher.clear_snapshot();
                return AppleDiscoveryExit::RuntimeDropped;
            }
            Ok(CentralDiscoverySessionEnd::CapacityMismatch) => {}
            Err(mdns_error) => {
                crate::diagnostic_log::debug!(
                    "wifi-auto: Apple service discovery unavailable: {mdns_error:?}"
                );
            }
        }

        service_discovery_publisher.clear_snapshot();
        if service_discovery_publisher.participation() != DiscoveryParticipation::Central {
            continue;
        }

        let restart_trigger = tokio::select! {
            () = tokio::time::sleep(RETRY_INTERVAL) => DiscoveryRestartTrigger::RetryElapsed,
            participation_change = service_discovery_publisher.wait_for_participation_change() => {
                match participation_change {
                    Ok(
                        DiscoveryParticipation::Inactive
                        | DiscoveryParticipation::Satellite
                        | DiscoveryParticipation::Central,
                    ) => DiscoveryRestartTrigger::ParticipationChanged,
                    Err(DiscoveryLifecycleError::Closed) => {
                        DiscoveryRestartTrigger::RuntimeDropped
                    }
                }
            }
        };
        match restart_trigger {
            DiscoveryRestartTrigger::RetryElapsed
            | DiscoveryRestartTrigger::ParticipationChanged => {}
            DiscoveryRestartTrigger::RuntimeDropped => {
                return AppleDiscoveryExit::RuntimeDropped;
            }
        }
    }
}

async fn run_central_session(
    service_discovery_publisher: &mut ServiceDiscoveryPublisher,
) -> Result<CentralDiscoverySessionEnd, MdnsError> {
    let mut apple_discovery = loop {
        tokio::select! {
            apple_discovery = AppleServiceDiscoveryBackend::new() => {
                break apple_discovery?;
            }
            participation_change = service_discovery_publisher.wait_for_participation_change() => {
                match participation_change {
                    Ok(DiscoveryParticipation::Central) => {}
                    Ok(DiscoveryParticipation::Inactive) => {
                        return Ok(CentralDiscoverySessionEnd::BecameInactive);
                    }
                    Ok(DiscoveryParticipation::Satellite) => {
                        return Ok(CentralDiscoverySessionEnd::BecameSatellite);
                    }
                    Err(DiscoveryLifecycleError::Closed) => {
                        return Ok(CentralDiscoverySessionEnd::RuntimeDropped);
                    }
                }
            }
        }
    };
    crate::diagnostic_log::debug!("wifi-auto: Apple service discovery advertising and browsing");

    loop {
        tokio::select! {
            next_snapshot = apple_discovery.next_snapshot() => {
                let discovery_snapshot = next_snapshot?;
                match service_discovery_publisher.replace_snapshot(discovery_snapshot) {
                    SnapshotPublication::Published => {}
                    SnapshotPublication::NotCentral(DiscoveryParticipation::Inactive) => {
                        return Ok(CentralDiscoverySessionEnd::BecameInactive);
                    }
                    SnapshotPublication::NotCentral(DiscoveryParticipation::Satellite) => {
                        return Ok(CentralDiscoverySessionEnd::BecameSatellite);
                    }
                    SnapshotPublication::NotCentral(DiscoveryParticipation::Central) => {}
                    SnapshotPublication::CapacityMismatch { expected, actual } => {
                        crate::diagnostic_log::debug!(
                            "wifi-auto: Apple discovery capacity mismatch: expected={}, actual={}",
                            expected.get(),
                            actual.get()
                        );
                        return Ok(CentralDiscoverySessionEnd::CapacityMismatch);
                    }
                }
            }
            participation_change = service_discovery_publisher.wait_for_participation_change() => {
                match participation_change {
                    Ok(DiscoveryParticipation::Central) => {}
                    Ok(DiscoveryParticipation::Inactive) => {
                        return Ok(CentralDiscoverySessionEnd::BecameInactive);
                    }
                    Ok(DiscoveryParticipation::Satellite) => {
                        return Ok(CentralDiscoverySessionEnd::BecameSatellite);
                    }
                    Err(DiscoveryLifecycleError::Closed) => {
                        return Ok(CentralDiscoverySessionEnd::RuntimeDropped);
                    }
                }
            }
        }
    }
}

enum AppleDiscoveryExit {
    RuntimeDropped,
}

enum CentralDiscoverySessionEnd {
    BecameInactive,
    BecameSatellite,
    RuntimeDropped,
    CapacityMismatch,
}

enum DiscoveryRestartTrigger {
    RetryElapsed,
    ParticipationChanged,
    RuntimeDropped,
}
