use super::*;
use crate::{Connection, DisconnectionReason};
use prns_core::routing::links::LinkId;
use proptest::prelude::*;

fn paired(registry: &mut DeviceRegistry, seed: u8) -> DeviceId {
    let device = create(registry, "Remembered");
    let enrollment = begin(registry, device, seed);
    assert_eq!(
        registry.step(complete(enrollment, seed)),
        CompleteEnrollmentOutcome::Recorded { enrollment }
    );
    device
}

fn connect(registry: &mut DeviceRegistry, device: DeviceId) -> Connection {
    let outcome = registry.step(BeginConnection { device });
    let Ok(BeginConnectionOutcome::Connect { connection }) = outcome else {
        panic!("unexpected connection: {outcome:?}");
    };
    connection
}

fn confirmation(connection: Connection, seed: u8) -> ConfirmConnection {
    ConfirmConnection {
        connection,
        target: target(seed),
        link: LinkId::new([seed; 16]),
    }
}

fn end(connection: Connection, reason: DisconnectionReason) -> EndConnection {
    EndConnection { connection, reason }
}

#[test]
fn only_paired_devices_connect_and_confirmation_binds_the_expected_target() {
    let mut registry = registry(1);
    let device = create(&mut registry, "New");
    assert_eq!(
        registry.step(BeginConnection { device }),
        Ok(BeginConnectionOutcome::NotPaired { device })
    );
    let enrollment = begin(&mut registry, device, 1);
    assert_eq!(
        registry.step(BeginConnection { device }),
        Ok(BeginConnectionOutcome::NotPaired { device })
    );
    assert_eq!(
        read(&mut registry, device).connection,
        ConnectionState::NotConnected
    );
    assert_eq!(
        registry.step(complete(enrollment, 1)),
        CompleteEnrollmentOutcome::Recorded { enrollment }
    );
    let connection = connect(&mut registry, device);
    assert_eq!(connection.device(), device);
    assert_eq!(connection.target(), target(1));
    assert_eq!(
        registry.step(BeginConnection { device }),
        Ok(BeginConnectionOutcome::AlreadyConnecting { connection })
    );
    assert_eq!(
        registry.step(confirmation(connection, 2)),
        ConfirmConnectionOutcome::TargetMismatch {
            rejected: confirmation(connection, 2)
        }
    );
    assert_eq!(
        read(&mut registry, device).connection,
        ConnectionState::Connecting { connection }
    );
    assert_eq!(
        registry.step(confirmation(connection, 1)),
        ConfirmConnectionOutcome::Connected {
            connection,
            link: LinkId::new([1; 16])
        }
    );
    assert_eq!(
        registry.step(BeginConnection { device }),
        Ok(BeginConnectionOutcome::AlreadyConnected {
            connection,
            link: LinkId::new([1; 16])
        })
    );
    assert_eq!(
        registry.step(confirmation(connection, 2)),
        ConfirmConnectionOutcome::StaleConnection {
            rejected: confirmation(connection, 2)
        }
    );
    assert_eq!(
        read(&mut registry, device).connection,
        ConnectionState::Connected {
            connection,
            link: LinkId::new([1; 16])
        }
    );
}

#[test]
fn retries_preserve_failure_reasons_and_reject_superseded_callbacks() {
    let mut registry = registry(1);
    let device = paired(&mut registry, 1);
    for reason in [
        DisconnectionReason::Cancelled,
        DisconnectionReason::TimedOut,
        DisconnectionReason::TransportLost,
        DisconnectionReason::AuthenticationFailed,
    ] {
        let old = connect(&mut registry, device);
        assert_eq!(
            registry.step(end(old, reason)),
            EndConnectionOutcome::AttemptEnded {
                connection: old,
                reason
            }
        );
        assert_eq!(
            read(&mut registry, device).connection,
            ConnectionState::Disconnected {
                connection: old,
                reason
            }
        );
        assert_eq!(
            registry.step(confirmation(old, 1)),
            ConfirmConnectionOutcome::StaleConnection {
                rejected: confirmation(old, 1)
            }
        );
        assert_eq!(
            registry.step(end(old, reason)),
            EndConnectionOutcome::StaleConnection {
                rejected: end(old, reason)
            }
        );
        let active = connect(&mut registry, device);
        assert_ne!(active, old);
        assert_eq!(
            registry.step(confirmation(old, 1)),
            ConfirmConnectionOutcome::StaleConnection {
                rejected: confirmation(old, 1)
            }
        );
        assert_eq!(
            registry.step(end(old, reason)),
            EndConnectionOutcome::StaleConnection {
                rejected: end(old, reason)
            }
        );
        assert_eq!(
            read(&mut registry, device).connection,
            ConnectionState::Connecting { connection: active }
        );
        assert_eq!(
            registry.step(confirmation(active, 1)),
            ConfirmConnectionOutcome::Connected {
                connection: active,
                link: LinkId::new([1; 16])
            }
        );
        assert_eq!(
            registry.step(end(old, reason)),
            EndConnectionOutcome::StaleConnection {
                rejected: end(old, reason)
            }
        );
        assert_eq!(
            read(&mut registry, device).connection,
            ConnectionState::Connected {
                connection: active,
                link: LinkId::new([1; 16])
            }
        );
        assert_eq!(
            registry.step(end(active, reason)),
            EndConnectionOutcome::SessionEnded {
                connection: active,
                link: LinkId::new([1; 16]),
                reason
            }
        );
        assert_eq!(
            read(&mut registry, device).connection,
            ConnectionState::Disconnected {
                connection: active,
                reason
            }
        );
        assert_eq!(
            read(&mut registry, device).enrollment,
            EnrollmentState::Paired {
                target: *target(1).public_keys()
            }
        );
    }
}

#[test]
fn forgetting_returns_pending_or_live_connection_and_invalidates_late_results() {
    let mut registry = registry(1);
    for connected in [false, true] {
        let device = paired(&mut registry, 1);
        let connection = connect(&mut registry, device);
        let previous = if connected {
            assert_eq!(
                registry.step(confirmation(connection, 1)),
                ConfirmConnectionOutcome::Connected {
                    connection,
                    link: LinkId::new([1; 16])
                }
            );
            ConnectionState::Connected {
                connection,
                link: LinkId::new([1; 16]),
            }
        } else {
            ConnectionState::Connecting { connection }
        };
        assert_eq!(
            registry.step(ForgetDevice { device }),
            ForgetDeviceOutcome::Forgotten {
                device,
                enrollment: EnrollmentState::Paired {
                    target: *target(1).public_keys()
                },
                connection: previous,
            }
        );
        let replacement = paired(&mut registry, 2);
        assert_eq!(
            registry.step(BeginConnection { device }),
            Ok(BeginConnectionOutcome::MissingDevice { device })
        );
        assert_eq!(
            registry.step(confirmation(connection, 1)),
            ConfirmConnectionOutcome::MissingDevice {
                rejected: confirmation(connection, 1)
            }
        );
        assert_eq!(
            registry.step(end(connection, DisconnectionReason::TransportLost)),
            EndConnectionOutcome::MissingDevice {
                rejected: end(connection, DisconnectionReason::TransportLost)
            }
        );
        assert_eq!(
            read(&mut registry, replacement).connection,
            ConnectionState::NotConnected
        );
        assert_eq!(
            registry.step(ForgetDevice {
                device: replacement
            }),
            ForgetDeviceOutcome::Forgotten {
                device: replacement,
                enrollment: EnrollmentState::Paired {
                    target: *target(2).public_keys()
                },
                connection: ConnectionState::NotConnected,
            }
        );
    }
}

#[test]
fn connection_generations_exhaust_without_wrapping_or_disturbing_other_devices() {
    let mut registry = registry(2);
    let first = paired(&mut registry, 1);
    let second = paired(&mut registry, 2);
    let first_connection = connect(&mut registry, first);
    registry.next_connection = Some(NonZeroU64::MAX);
    let last = connect(&mut registry, second);
    assert_eq!(last.generation, NonZeroU64::MAX);
    assert_eq!(
        registry.step(end(last, DisconnectionReason::Cancelled)),
        EndConnectionOutcome::AttemptEnded {
            connection: last,
            reason: DisconnectionReason::Cancelled
        }
    );
    assert_eq!(
        registry.step(BeginConnection { device: second }),
        Err(BeginConnectionError::IdentifiersExhausted { device: second })
    );
    assert_eq!(
        read(&mut registry, second).connection,
        ConnectionState::Disconnected {
            connection: last,
            reason: DisconnectionReason::Cancelled
        }
    );
    assert_eq!(
        read(&mut registry, first).connection,
        ConnectionState::Connecting {
            connection: first_connection
        }
    );
    assert_eq!(
        registry.step(confirmation(first_connection, 1)),
        ConfirmConnectionOutcome::Connected {
            connection: first_connection,
            link: LinkId::new([1; 16])
        }
    );
}

proptest! {
    #[test]
    fn arbitrary_reconnect_histories_isolate_sessions_and_preserve_pairing(
        rounds in prop::collection::vec((any::<bool>(), any::<bool>()), 1..40),
    ) {
        let mut registry = registry(2);
        let first = paired(&mut registry, 1);
        let second = paired(&mut registry, 2);
        let other = connect(&mut registry, second);
        let mut retired = alloc::vec::Vec::new();
        for (confirm, cancel) in rounds {
            let active = connect(&mut registry, first);
            prop_assert!(!retired.contains(&active));
            for &old in &retired {
                prop_assert_eq!(registry.step(confirmation(old, 1)), ConfirmConnectionOutcome::StaleConnection { rejected: confirmation(old, 1) });
                prop_assert_eq!(registry.step(end(old, DisconnectionReason::TransportLost)), EndConnectionOutcome::StaleConnection { rejected: end(old, DisconnectionReason::TransportLost) });
            }
            prop_assert_eq!(read(&mut registry, first).connection, ConnectionState::Connecting { connection: active });
            if confirm {
                prop_assert_eq!(registry.step(confirmation(active, 1)), ConfirmConnectionOutcome::Connected { connection: active, link: LinkId::new([1; 16]) });
            }
            let reason = if cancel { DisconnectionReason::Cancelled } else { DisconnectionReason::TimedOut };
            let expected = if confirm {
                EndConnectionOutcome::SessionEnded { connection: active, link: LinkId::new([1; 16]), reason }
            } else {
                EndConnectionOutcome::AttemptEnded { connection: active, reason }
            };
            prop_assert_eq!(registry.step(end(active, reason)), expected);
            prop_assert_eq!(read(&mut registry, first).connection, ConnectionState::Disconnected { connection: active, reason });
            prop_assert_eq!(read(&mut registry, first).enrollment, EnrollmentState::Paired { target: *target(1).public_keys() });
            prop_assert_eq!(read(&mut registry, second).connection, ConnectionState::Connecting { connection: other });
            retired.push(active);
        }
    }
}
