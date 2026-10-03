use super::*;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn arbitrary_cancellation_histories_preserve_link_ownership(operations in prop::collection::vec((0u8..3, 0usize..3, 0u8..3), 0..30)) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let (mut registry, first, backend) = fixture();
            let shared = backend.shared.clone();
            let mut connections = alloc::vec![first];
            for _ in 0..3 {
                let current = *connections.last().unwrap();
                let _ended = registry.step(EndConnection { connection: current, reason: DisconnectionReason::Cancelled });
                connections.push(begin(&mut registry, first.device()));
            }
            let barrier = *connections.last().unwrap();
            let (mut worker, run) = PrnsDeviceWorker::try_new(first.device(), backend, NonZeroUsize::new(2).unwrap()).unwrap();
            let mut active = None;
            let mut expected_calls = alloc::vec::Vec::new();
            bounded(async {
                tokio::join!(run, async {
                    for (operation, index, cancellation) in operations {
                        let connection = *connections.get(index).unwrap();
                        let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
                        let input = match operation {
                            0 => PrnsDeviceIn::Connect { connection },
                            1 => PrnsDeviceIn::Inventory { request },
                            _ => PrnsDeviceIn::Close { connection },
                        };
                        let was_active = active;
                        let mut pending = submit(&mut worker, input);
                        if cancellation == 0 {
                            drop(pending);
                        } else {
                            let output = if cancellation == 1 {
                                let output = (&mut pending.result).await.unwrap().unwrap();
                                drop(pending);
                                output
                            } else {
                                receive(pending).await
                            };
                            match operation {
                                0 => {
                                    let expected = match was_active {
                                        None => PrnsDeviceOut::Connected { confirmation: ConfirmConnection { connection, target: connection.target(), link: LINK } },
                                        Some(current) if current == connection => PrnsDeviceOut::AlreadyConnected { connection, link: LINK },
                                        Some(current) => PrnsDeviceOut::Busy { active: current, rejected: connection },
                                    };
                                    assert_eq!(output, expected);
                                }
                                1 if was_active == Some(connection) => {
                                    let PrnsDeviceOut::InterfacesReceived { response, rtt } = output else { panic!("inventory missing") };
                                    assert_eq!(response.request, request);
                                    assert_eq!(rtt, RttMillis::new(42));
                                }
                                1 => assert_eq!(output, PrnsDeviceOut::StaleInventory { request }),
                                _ if was_active == Some(connection) => assert_eq!(output, PrnsDeviceOut::Closed { connection, settlement: CloseRemoteControlTargetOutcome::Queued }),
                                _ => assert_eq!(output, PrnsDeviceOut::StaleClose { connection }),
                            }
                        }
                        match operation {
                            0 if cancellation != 0 && was_active.is_none() => {
                                expected_calls.extend([
                                    Call::Resolve(connection.target().identity_hash()),
                                    Call::Establish(connection.target().endpoint().destination_hash()),
                                    Call::Identify(LINK, controller().identity_hash()),
                                ]);
                                if cancellation == 1 {
                                    expected_calls.push(Call::Close(LINK));
                                } else {
                                    active = Some(connection);
                                }
                            }
                            1 if cancellation != 0 && was_active == Some(connection) => expected_calls.push(Call::Inventory(LINK, RemoteControlInterfacePage::First)),
                            2 if was_active == Some(connection) => {
                                expected_calls.push(Call::Close(LINK));
                                active = None;
                            }
                            _ => {}
                        }
                        assert_eq!(receive(submit(&mut worker, PrnsDeviceIn::Close { connection: barrier })).await, PrnsDeviceOut::StaleClose { connection: barrier });
                        assert_eq!(*shared.calls.lock().unwrap(), expected_calls);
                    }
                    drop(worker);
                });
            }).await;
            if active.is_some() { expected_calls.push(Call::Close(LINK)); }
            assert_eq!(*shared.calls.lock().unwrap(), expected_calls);
        });
    }

    #[test]
    fn arbitrary_queue_bounds_retain_every_rejected_command(capacity in 1usize..12) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let (_, connection, backend) = fixture();
            let shared = backend.shared.clone();
            let (mut worker, run) = PrnsDeviceWorker::try_new(connection.device(), backend, NonZeroUsize::new(capacity).unwrap()).unwrap();
            let mut accepted = alloc::vec::Vec::new();
            for _ in 0..capacity {
                accepted.push(submit(&mut worker, PrnsDeviceIn::Close { connection }));
            }
            let (_, mut outgoing) = <_ as DuplexFitting<PrnsDevice>>::split(&mut worker);
            let PrnsDeviceSubmission::Busy { input } = outgoing.send(PrnsDeviceIn::Connect { connection }) else { panic!("unbounded queue") };
            assert_eq!(input, PrnsDeviceIn::Connect { connection });
            drop(worker);
            bounded(async {
                tokio::join!(run, async {
                    for pending in accepted {
                        assert_eq!(receive(pending).await, PrnsDeviceOut::StaleClose { connection });
                    }
                });
            }).await;
            assert!(shared.calls.lock().unwrap().is_empty());
        });
    }
}
