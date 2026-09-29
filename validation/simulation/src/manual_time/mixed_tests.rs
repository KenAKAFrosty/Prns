use super::*;
use crate::ble::*;
use crate::{
    AdvanceReport, FaultPlan, SimulationDurationInTicks, TopologyConfig, TransmissionOrdinal,
    TransmissionRule, VirtualMedium, VirtualMediumConfig,
};
use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, RadioMode, ScanningMode, BLE_HW_MTU, CONTROL_MAX_LEN,
};
use std::num::NonZeroUsize;
use std::pin::pin;

fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

fn checked<T>(value: Result<T, ManualTimeError>) -> T {
    value.unwrap_or_else(|error| unreachable!("mixed clock: {error}"))
}

fn media(emissions: usize) -> (VirtualMedium, VirtualBleLab, Vec<VirtualBleBackend>) {
    let frames = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::FullyConnected,
            2,
            2,
            2,
            64,
            FaultPlan::new(vec![TransmissionRule::delay(
                TransmissionOrdinal::new(0),
                SimulationDurationInTicks::from_ticks(5),
            )])
            .unwrap(),
        )
        .unwrap(),
    );
    let ble = VirtualBleLab::new(
        BleMediumConfig::new(TopologyConfig::FullyConnected, 2, 4, emissions, 64).unwrap(),
    );
    let backends = (1..=2)
        .map(|address| {
            ble.attach_backend(
                VirtualBleBackendConfig::new(
                    BleAddress::new([address; 6]),
                    -40,
                    BleRoleCapabilities::DualRole,
                    SimulationDurationInTicks::from_ticks(5),
                    VirtualBleBackendLimits {
                        inbound_links: NonZeroUsize::MIN,
                        connections: NonZeroUsize::MIN,
                        discovered_peers: NonZeroUsize::MIN,
                    },
                    VirtualBleLinkConfig::new(
                        1,
                        1,
                        BLE_HW_MTU,
                        VirtualGattConfig::new(CONTROL_MAX_LEN, 20).unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )
            .unwrap()
        })
        .collect();
    (frames, ble, backends)
}

fn driver(frames: &VirtualMedium, ble: &VirtualBleLab) -> ManualTimeDriver {
    checked(ManualTimeDriver::new(
        ManualMedium::FramesAndBle {
            frames: frames.clone(),
            ble: ble.clone(),
        },
        Duration::from_millis(2),
    ))
}

fn advertising(
    driver: &mut ManualTimeDriver,
    backend: &mut VirtualBleBackend,
    mode: AdvertisingMode,
) {
    let mut setup = pin!(async {
        BleBackend::<1>::set_radio_mode(backend, RadioMode::On)
            .await
            .unwrap();
        BleBackend::<1>::set_advertising(backend, mode)
            .await
            .unwrap();
    });
    assert_eq!(checked(driver.poll(setup.as_mut())), Poll::Ready(()));
}

#[test]
fn mixed_same_tick_media_settle_before_the_runtime_timer() {
    let (frames, ble, mut backends) = media(2);
    let sender = frames.attach(b"sender").unwrap();
    let _receiver = frames.attach(b"receiver").unwrap();
    let mut driver = driver(&frames, &ble);
    advertising(&mut driver, &mut backends[0], AdvertisingMode::On);
    let mut scan = pin!(async {
        BleBackend::<1>::set_radio_mode(&mut backends[1], RadioMode::On)
            .await
            .unwrap();
        BleBackend::<1>::set_scanning(&mut backends[1], ScanningMode::On)
            .await
            .unwrap();
    });
    assert_eq!(checked(driver.poll(scan.as_mut())), Poll::Ready(()));
    let initial = checked(driver.advance_to_next_event(tick(100)));
    assert_eq!(initial.bounds(), (tick(0), tick(0)));
    assert_eq!(frames.transmit(sender.endpoint_id(), vec![1, 2, 3]), Ok(()));
    let mut waiting = pin!(async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        (frames.now(), ble.now(), frames.pending_delivery_count())
    });
    assert_eq!(checked(driver.poll(waiting.as_mut())), Poll::Pending);
    assert_eq!(
        checked(driver.advance_to_next_event(tick(3))).bounds(),
        (tick(0), tick(3))
    );
    assert_eq!(checked(driver.poll(waiting.as_mut())), Poll::Pending);
    assert_eq!(
        checked(driver.advance_to_next_event(tick(100))),
        ManualAdvance::FramesAndBle {
            frames: AdvanceReport {
                from: tick(3),
                to: tick(5),
                receptions_queued: 1,
                receptions_dropped: 0
            },
            ble: BleAdvanceReport {
                from: tick(3),
                to: tick(5),
                advertisements_emitted: 1,
                observations_queued: 1,
                observations_dropped: 0
            },
        }
    );
    assert_eq!(
        checked(driver.poll(waiting.as_mut())),
        Poll::Ready((tick(5), tick(5), 0))
    );
    assert_eq!(
        checked(driver.snapshot()),
        ManualTimeSnapshot {
            tick: tick(5),
            runtime_elapsed: Duration::from_millis(10)
        }
    );
}

#[test]
fn mixed_emission_refusal_changes_neither_medium_nor_runtime_and_is_retryable() {
    let (frames, ble, mut backends) = media(1);
    let sender = frames.attach(b"sender").unwrap();
    let _receiver = frames.attach(b"receiver").unwrap();
    let mut driver = driver(&frames, &ble);
    advertising(&mut driver, &mut backends[0], AdvertisingMode::On);
    assert_eq!(
        checked(driver.advance_to_next_event(tick(5))).bounds(),
        (tick(0), tick(0))
    );
    assert_eq!(
        checked(driver.advance_to_next_event(tick(5))).bounds(),
        (tick(0), tick(5))
    );
    advertising(&mut driver, &mut backends[1], AdvertisingMode::On);
    assert_eq!(
        checked(driver.advance_to_next_event(tick(5))).bounds(),
        (tick(5), tick(5))
    );
    assert_eq!(frames.transmit(sender.endpoint_id(), vec![7]), Ok(()));
    let before = (frames.trace(), ble.trace(), checked(driver.snapshot()));
    assert!(matches!(
        driver.advance_to_next_event(tick(10)),
        Err(ManualTimeError::Ble(
            BleAdvanceError::EmissionBudgetExceeded { maximum: 1 }
        ))
    ));
    assert_eq!(
        (frames.trace(), ble.trace(), checked(driver.snapshot())),
        before
    );
    advertising(&mut driver, &mut backends[1], AdvertisingMode::Off);
    assert_eq!(frames.pending_delivery_count(), 1);
    assert_eq!(
        checked(driver.advance_to_next_event(tick(10))).bounds(),
        (tick(5), tick(10))
    );
    assert_eq!(frames.pending_delivery_count(), 0);
    assert_eq!((frames.now(), ble.now()), (tick(10), tick(10)));
}

#[test]
fn mixed_construction_rejects_unequal_origins_without_mutation() {
    let (frames, ble, _backends) = media(2);
    frames.advance_to(tick(7)).unwrap();
    let before = (frames.trace(), ble.trace());
    let result = ManualTimeDriver::new(
        ManualMedium::FramesAndBle {
            frames: frames.clone(),
            ble: ble.clone(),
        },
        Duration::from_millis(1),
    );
    assert!(
        matches!(result, Err(ManualTimeError::MediaDisagree { frames, ble }) if frames == tick(7) && ble == tick(0))
    );
    assert_eq!((frames.trace(), ble.trace()), before);
}

#[test]
fn mixed_external_drift_refuses_observation_polling_and_advancement_before_effects() {
    for drift in [Drift::Frames, Drift::Ble, Drift::Both] {
        let (frames, ble, _backends) = media(2);
        let mut driver = driver(&frames, &ble);
        match drift {
            Drift::Frames => {
                frames.advance_to(tick(1)).unwrap();
            }
            Drift::Ble => {
                ble.advance_to(tick(1)).unwrap();
            }
            Drift::Both => {
                frames.advance_to(tick(1)).unwrap();
                ble.advance_to(tick(1)).unwrap();
            }
        }
        let before = (frames.trace(), ble.trace());
        let mut polled = false;
        let poll_result = driver.poll(Box::pin(async { polled = true }).as_mut());
        assert!(!polled);
        for result in [
            driver.snapshot().map(|_| ()),
            poll_result.map(|_| ()),
            driver.advance_to_next_event(tick(2)).map(|_| ()),
        ] {
            match drift {
                Drift::Frames | Drift::Ble => {
                    assert!(matches!(result, Err(ManualTimeError::MediaDisagree { .. })))
                }
                Drift::Both => assert!(
                    matches!(result, Err(ManualTimeError::MediumDrift { expected, observed }) if expected == tick(0) && observed == tick(1))
                ),
            }
        }
        assert_eq!((frames.trace(), ble.trace()), before);
    }
}

enum Drift {
    Frames,
    Ble,
    Both,
}

#[test]
fn wake_stepping_preflights_ble_budget_before_either_clock_moves() {
    let (frames, ble, mut backends) = media(1);
    let mut driver = checked(ManualTimeDriver::new(
        ManualMedium::FramesAndBle {
            frames: frames.clone(),
            ble: ble.clone(),
        },
        Duration::from_millis(1),
    ));
    for backend in &mut backends {
        advertising(&mut driver, backend, AdvertisingMode::On);
    }
    let before = (frames.trace(), ble.trace(), checked(driver.snapshot()));
    let mut runner = ManualTaskRunner::<()>::new(&mut driver, NonZeroUsize::MIN);
    assert!(matches!(
        runner.advance_to_next_wake(tick(100)),
        Err(ManualTimeError::Ble(_))
    ));
    assert_eq!(
        (frames.trace(), ble.trace(), checked(runner.snapshot())),
        before
    );
}

#[test]
fn wake_stepping_refuses_coarse_ticks_without_mutation() {
    let (frames, ble, _backends) = media(2);
    let mut driver = driver(&frames, &ble);
    let before = (frames.trace(), ble.trace(), checked(driver.snapshot()));
    let mut runner = ManualTaskRunner::<()>::new(&mut driver, NonZeroUsize::MIN);
    assert!(matches!(
        runner.advance_to_next_wake(tick(100)),
        Err(ManualTimeError::WakeSteppingRequiresMillisecondTicks)
    ));
    assert_eq!(
        (frames.trace(), ble.trace(), checked(runner.snapshot())),
        before
    );
}

#[test]
fn mixed_advance_selects_each_medium_when_its_deadline_is_earliest() {
    let (frames, ble, mut backends) = media(2);
    let sender = frames.attach(b"sender").unwrap();
    let _receiver = frames.attach(b"receiver").unwrap();
    let mut driver = driver(&frames, &ble);
    assert_eq!(frames.transmit(sender.endpoint_id(), vec![9]), Ok(()));
    assert_eq!(
        checked(driver.advance_to_next_event(tick(2))).bounds(),
        (tick(0), tick(2))
    );
    advertising(&mut driver, &mut backends[0], AdvertisingMode::On);
    assert_eq!(
        checked(driver.advance_to_next_event(tick(100))).bounds(),
        (tick(2), tick(2))
    );
    assert_eq!(
        checked(driver.advance_to_next_event(tick(100))),
        ManualAdvance::FramesAndBle {
            frames: AdvanceReport {
                from: tick(2),
                to: tick(5),
                receptions_queued: 1,
                receptions_dropped: 0
            },
            ble: BleAdvanceReport {
                from: tick(2),
                to: tick(5),
                advertisements_emitted: 0,
                observations_queued: 0,
                observations_dropped: 0
            },
        }
    );
    assert_eq!(
        checked(driver.advance_to_next_event(tick(100))),
        ManualAdvance::FramesAndBle {
            frames: AdvanceReport {
                from: tick(5),
                to: tick(7),
                receptions_queued: 0,
                receptions_dropped: 0
            },
            ble: BleAdvanceReport {
                from: tick(5),
                to: tick(7),
                advertisements_emitted: 1,
                observations_queued: 0,
                observations_dropped: 0
            },
        }
    );
    assert_eq!((frames.now(), ble.now()), (tick(7), tick(7)));
}

#[test]
fn mixed_clock_range_refusal_precedes_both_medium_advances() {
    let (frames, ble, _backends) = media(2);
    let mut driver = driver(&frames, &ble);
    let before = (frames.trace(), ble.trace(), checked(driver.snapshot()));
    assert!(matches!(
        driver.advance_to_next_event(tick(u64::MAX)),
        Err(ManualTimeError::ClockRange { .. })
    ));
    assert_eq!(
        (frames.trace(), ble.trace(), checked(driver.snapshot())),
        before
    );
}

#[test]
fn mixed_nonzero_origin_and_backward_refusal_preserve_both_timelines() {
    let (frames, ble, _backends) = media(2);
    frames.advance_to(tick(37)).unwrap();
    ble.advance_to(tick(37)).unwrap();
    let mut driver = driver(&frames, &ble);
    assert_eq!(
        checked(driver.advance_to_next_event(tick(41))).bounds(),
        (tick(37), tick(41))
    );
    assert_eq!(
        checked(driver.snapshot()),
        ManualTimeSnapshot {
            tick: tick(41),
            runtime_elapsed: Duration::from_millis(8)
        }
    );
    let before = (frames.trace(), ble.trace());
    assert!(matches!(
        driver.advance_to_next_event(tick(40)),
        Err(ManualTimeError::BeforeCurrent { .. })
    ));
    assert_eq!((frames.trace(), ble.trace()), before);
}
