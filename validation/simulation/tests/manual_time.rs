#![cfg(feature = "controlled-time")]

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::{pin, Pin};
use std::task::Poll;
use std::time::Duration;

use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, AppleHost, BleAddress, BleBackend, BleEvent, BleIdentity, BleLink,
    BleRoleCapabilities, Endpoint, LinkCapabilities, Origin, RadioMode, BLE_HW_MTU,
    CONTROL_MAX_LEN,
};
use personal_rns::interfaces::{ConnectionState, InterfaceStatus};
use personal_rns::runtime::{Fleet, InterfaceSupervisor};
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::ble::{
    BleAdvanceError, BleAdvanceReport, BleMediumConfig, VirtualBleBackend, VirtualBleBackendConfig,
    VirtualBleBackendLimits, VirtualBleError, VirtualBleLab, VirtualBleLinkConfig,
    VirtualGattConfig,
};
use prns_simulation::{
    ManualAdvance, ManualMedium, ManualTimeDriver, ManualTimeError, ManualTimeSnapshot,
    SimulationDurationInTicks, SimulationTick, TopologyConfig,
};

const MAX_PEERS: usize = 4;

fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

fn checked<T>(value: Result<T, ManualTimeError>) -> T {
    value.unwrap_or_else(|error| unreachable!("manual driving succeeds: {error}"))
}

fn ready<F: Future>(driver: &mut ManualTimeDriver, future: Pin<&mut F>) -> F::Output {
    match checked(driver.poll(future)) {
        Poll::Ready(value) => value,
        Poll::Pending => unreachable!("this operation must be immediately ready"),
    }
}

struct Scenario {
    lab: VirtualBleLab,
    driver: ManualTimeDriver,
    supervisor: BluetoothAuto<VirtualBleBackend, MAX_PEERS>,
    remote: VirtualBleBackend,
}

fn scenario(emission_budget: usize) -> Scenario {
    let config = BleMediumConfig::new(TopologyConfig::FullyConnected, 2, 4, emission_budget, 64)
        .unwrap_or_else(|error| unreachable!("valid medium: {error}"));
    let lab = VirtualBleLab::new(config);
    let backend = |address| {
        let gatt = VirtualGattConfig::new(CONTROL_MAX_LEN, 20)
            .unwrap_or_else(|error| unreachable!("valid GATT limits: {error}"));
        let link = VirtualBleLinkConfig::new(4, 4, BLE_HW_MTU, gatt)
            .unwrap_or_else(|error| unreachable!("valid link limits: {error}"));
        let config = VirtualBleBackendConfig::new(
            BleAddress::new([address; 6]),
            -40,
            BleRoleCapabilities::DualRole,
            SimulationDurationInTicks::from_ticks(60_000),
            VirtualBleBackendLimits {
                inbound_links: NonZeroUsize::MIN,
                connections: NonZeroUsize::MIN,
                discovered_peers: NonZeroUsize::MIN,
            },
            link,
        )
        .unwrap_or_else(|error| unreachable!("valid backend: {error}"));
        lab.attach_backend(config)
            .unwrap_or_else(|error| unreachable!("unique address: {error}"))
    };
    let supervisor = BluetoothAuto::new(
        backend(1),
        BleIdentity::new([1; 16]),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
    );
    let mut remote = backend(2);
    let mut driver = checked(ManualTimeDriver::new(
        ManualMedium::Ble(lab.clone()),
        Duration::from_millis(1),
    ));
    ready(
        &mut driver,
        pin!(async {
            assert!(
                BleBackend::<MAX_PEERS>::set_radio_mode(&mut remote, RadioMode::On)
                    .await
                    .is_ok()
            );
            assert!(
                BleBackend::<MAX_PEERS>::set_advertising(&mut remote, AdvertisingMode::On)
                    .await
                    .is_ok()
            );
        }),
    );
    Scenario {
        lab,
        driver,
        supervisor,
        remote,
    }
}

#[test]
fn production_supervisor_times_out_on_the_coordinated_clock() {
    let Scenario {
        lab,
        mut driver,
        supervisor,
        mut remote,
    } = scenario(2);
    let status = supervisor.status();
    let (fleet, _detached) = Fleet::detached(status.id());
    let mut running = pin!(supervisor.run(fleet));
    assert_eq!(checked(driver.poll(running.as_mut())), Poll::Pending);
    assert_eq!(
        checked(driver.advance_to_next_event(tick(9_999))),
        ManualAdvance::Ble(BleAdvanceReport {
            from: tick(0),
            to: tick(0),
            advertisements_emitted: 2,
            observations_queued: 1,
            observations_dropped: 0,
        })
    );
    assert_eq!(checked(driver.poll(running.as_mut())), Poll::Pending);
    let BleEvent::LinkReady {
        mut link,
        origin: Origin::Accepted,
        ..
    } = ready(
        &mut driver,
        pin!(BleBackend::<MAX_PEERS>::next_event(&mut remote)),
    )
    else {
        unreachable!("the supervisor dials the remote")
    };
    let _ = checked(driver.advance_to_next_event(tick(9_999)));
    assert_eq!(checked(driver.poll(running.as_mut())), Poll::Pending);
    assert_eq!(
        (lab.active_connection_count(), checked(driver.snapshot())),
        (
            1,
            ManualTimeSnapshot {
                tick: tick(9_999),
                runtime_elapsed: Duration::from_millis(9_999)
            }
        )
    );

    let _ = checked(driver.advance_to_next_event(tick(10_000)));
    assert_eq!(checked(driver.poll(running.as_mut())), Poll::Pending);
    assert_eq!(
        (
            lab.active_connection_count(),
            status.connection(),
            checked(driver.snapshot())
        ),
        (
            0,
            ConnectionState::Disconnected,
            ManualTimeSnapshot {
                tick: tick(10_000),
                runtime_elapsed: Duration::from_secs(10)
            }
        )
    );
    assert_eq!(
        ready(&mut driver, pin!(link.control_recv())),
        Err(VirtualBleError::LinkClosed)
    );
}

#[test]
fn a_refused_medium_step_does_not_advance_tokio_and_can_be_retried() {
    let Scenario {
        lab,
        mut driver,
        supervisor,
        mut remote,
    } = scenario(1);
    let status = supervisor.status();
    let (fleet, _detached) = Fleet::detached(status.id());
    let mut running = pin!(supervisor.run(fleet));
    assert_eq!(checked(driver.poll(running.as_mut())), Poll::Pending);
    let before = (checked(driver.snapshot()), lab.trace());
    assert!(matches!(
        driver.advance_to_next_event(tick(10_000)),
        Err(ManualTimeError::Ble(
            BleAdvanceError::EmissionBudgetExceeded { maximum: 1 }
        ))
    ));
    assert_eq!((checked(driver.snapshot()), lab.trace()), before);

    assert!(ready(
        &mut driver,
        pin!(BleBackend::<MAX_PEERS>::set_advertising(
            &mut remote,
            AdvertisingMode::Off
        ))
    )
    .is_ok());
    assert_eq!(
        checked(driver.advance_to_next_event(tick(10_000))),
        ManualAdvance::Ble(BleAdvanceReport {
            from: tick(0),
            to: tick(0),
            advertisements_emitted: 1,
            observations_queued: 0,
            observations_dropped: 0,
        })
    );
    let _ = checked(driver.advance_to_next_event(tick(10_000)));
    assert_eq!(
        checked(driver.snapshot()),
        ManualTimeSnapshot {
            tick: tick(10_000),
            runtime_elapsed: Duration::from_secs(10),
        }
    );
}
