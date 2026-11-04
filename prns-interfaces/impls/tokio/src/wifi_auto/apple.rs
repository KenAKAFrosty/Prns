use std::time::Duration;

use prns_core::interfaces::wifi_auto::{
    EphemeralDiscoveryInstanceName, EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES,
};
use prns_ffi::mdns::macos::{AppleServiceDiscoveryBackend, MdnsError, DISCOVERY_CAPACITY};

use super::discovery::DiscoveryDiagnostic;
use super::{
    DiscoveryLifecycleError, DiscoveryParticipation, ServiceDiscovery, ServiceDiscoveryPublisher,
    ServiceDiscoveryTask, SnapshotPublication,
};

const RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Starts Apple Network Services behind a bounded AutoWifi discovery channel.
///
/// The native run loop exists only while the associated AutoWifi runtime is
/// the local [`DiscoveryParticipation::Central`].
pub fn apple_service_discovery() -> ServiceDiscovery {
    let (service_discovery, service_discovery_publisher) =
        ServiceDiscovery::channel(DISCOVERY_CAPACITY);
    tokio::spawn(run_service_discovery(
        service_discovery_publisher,
        DiscoveryDiagnostic::default(),
    ));
    service_discovery
}

/// The same Apple provider with a joinable owner for replaceable host interfaces.
pub fn apple_service_discovery_managed() -> (ServiceDiscovery, ServiceDiscoveryTask) {
    let (service_discovery, publisher) = ServiceDiscovery::channel(DISCOVERY_CAPACITY);
    let task = ServiceDiscoveryTask::spawn_reporting(move |diagnostic| async move {
        let _ = run_service_discovery(publisher, diagnostic).await;
    });
    (service_discovery, task)
}

async fn run_service_discovery(
    mut service_discovery_publisher: ServiceDiscoveryPublisher,
    diagnostic: DiscoveryDiagnostic,
) -> AppleDiscoveryExit {
    let mut network_changes = service_discovery_publisher.subscribe_network_changes();
    loop {
        diagnostic.set_active(false);
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

        let session_result =
            run_central_session(&mut service_discovery_publisher, &diagnostic).await;
        diagnostic.set_active(false);
        match session_result {
            Ok(
                CentralDiscoverySessionEnd::BecameInactive
                | CentralDiscoverySessionEnd::BecameSatellite
                | CentralDiscoverySessionEnd::NetworkChanged,
            ) => {
                service_discovery_publisher.clear_snapshot();
                continue;
            }
            Ok(CentralDiscoverySessionEnd::RuntimeDropped) => {
                service_discovery_publisher.clear_snapshot();
                return AppleDiscoveryExit::RuntimeDropped;
            }
            Ok(CentralDiscoverySessionEnd::CapacityMismatch) => {
                diagnostic.set(Some(
                    "Bonjour discovery returned an incompatible advertisement capacity.".into(),
                ));
            }
            Err(apple_discovery_error) => {
                diagnostic.set(Some(format!("Bonjour discovery is unavailable: {apple_discovery_error}. Discovery will retry.")));
                crate::diagnostic_log::debug!(
                    "wifi-auto: Apple service discovery unavailable: {apple_discovery_error}"
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
            _ = network_changes.changed() => DiscoveryRestartTrigger::ParticipationChanged,
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
    diagnostic: &DiscoveryDiagnostic,
) -> Result<CentralDiscoverySessionEnd, AppleDiscoveryError> {
    let mut network_changes = service_discovery_publisher.subscribe_network_changes();
    let tcp_instance_name = fresh_instance_name()?;
    let udp_instance_name = fresh_instance_name()?;
    let mut apple_discovery = loop {
        tokio::select! {
            _ = network_changes.changed() => { return Ok(CentralDiscoverySessionEnd::NetworkChanged); }
            apple_discovery = AppleServiceDiscoveryBackend::new(
                tcp_instance_name.clone(),
                udp_instance_name.clone(),
            ) => {
                break apple_discovery.map_err(AppleDiscoveryError::Native)?;
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
    diagnostic.set(None);
    diagnostic.set_active(true);
    crate::diagnostic_log::debug!("wifi-auto: Apple service discovery advertising and browsing");

    loop {
        tokio::select! {
            _ = network_changes.changed() => {
                return Ok(CentralDiscoverySessionEnd::NetworkChanged);
            }
            next_snapshot = apple_discovery.next_snapshot() => {
                let discovery_snapshot = next_snapshot.map_err(AppleDiscoveryError::Native)?;
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

fn fresh_instance_name() -> Result<EphemeralDiscoveryInstanceName, AppleDiscoveryError> {
    let mut random_bytes = [0u8; EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES];
    getrandom::getrandom(&mut random_bytes).map_err(AppleDiscoveryError::RandomnessUnavailable)?;
    Ok(EphemeralDiscoveryInstanceName::from_random_bytes(
        random_bytes,
    ))
}

#[derive(Debug)]
enum AppleDiscoveryError {
    Native(MdnsError),
    RandomnessUnavailable(getrandom::Error),
}

impl std::fmt::Display for AppleDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Native(mdns_error) => write!(formatter, "native backend: {mdns_error}"),
            Self::RandomnessUnavailable(randomness_error) => {
                write!(
                    formatter,
                    "ephemeral publication randomness: {randomness_error}"
                )
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
    NetworkChanged,
}

enum DiscoveryRestartTrigger {
    RetryElapsed,
    ParticipationChanged,
    RuntimeDropped,
}
