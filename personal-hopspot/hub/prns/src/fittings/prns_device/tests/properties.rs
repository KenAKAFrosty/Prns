use super::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_commands_match_single_link_ownership(operations in proptest::collection::vec((0u8..3, 0usize..3), 0..40)) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let (mut registry, first, backend) = fixture();
            let mut connections = alloc::vec![first];
            for _ in 0..2 {
                let connection = *connections.last().unwrap();
                assert_eq!(registry.step(EndConnection { connection, reason: DisconnectionReason::Cancelled }), EndConnectionOutcome::AttemptEnded { connection, reason: DisconnectionReason::Cancelled });
                connections.push(begin(&mut registry, first.device()));
            }
            let shared = Arc::clone(&backend.shared);
            let mut fitting = PrnsDeviceFitting::new(first.device(), backend);
            let mut active = None;
            let mut calls = alloc::vec::Vec::new();
            for (operation, index) in operations {
                let connection = *connections.get(index).unwrap();
                match operation {
                    0 => {
                        let output = perform(&mut fitting, PrnsDeviceIn::Connect { connection }).await;
                        if let Some(current) = active {
                            assert_eq!(output, if current == connection { PrnsDeviceOut::AlreadyConnected { connection, link: LINK } } else { PrnsDeviceOut::Busy { active: current, rejected: connection } });
                        } else {
                            assert_eq!(output, PrnsDeviceOut::Connected { confirmation: ConfirmConnection { connection, target: connection.target(), link: LINK } });
                            active = Some(connection);
                            calls.extend([Call::Resolve(connection.target().identity_hash()), Call::Establish(connection.target().endpoint().destination_hash()), Call::Identify(LINK, controller().identity_hash())]);
                        }
                    }
                    1 => {
                        let output = perform(&mut fitting, PrnsDeviceIn::Close { connection }).await;
                        if active == Some(connection) {
                            assert_eq!(output, PrnsDeviceOut::Closed { connection, settlement: CloseRemoteControlTargetOutcome::Queued });
                            active = None;
                            calls.push(Call::Close(LINK));
                        } else { assert_eq!(output, PrnsDeviceOut::StaleClose { connection }); }
                    }
                    _ => {
                        let (_, request) = InterfaceInventory::<4>::with_initial_refresh(connection);
                        let output = perform(&mut fitting, PrnsDeviceIn::Inventory { request }).await;
                        if active == Some(connection) {
                            let PrnsDeviceOut::InterfacesReceived { response, rtt } = output else { panic!("inventory not received") };
                            assert_eq!(response.request, request);
                            assert_eq!(rtt, RttMillis::new(42));
                            calls.push(Call::Inventory(LINK, RemoteControlInterfacePage::First));
                        } else { assert_eq!(output, PrnsDeviceOut::StaleInventory { request }); }
                    }
                }
                assert_eq!(*shared.calls.lock().unwrap(), calls);
            }
            drop(fitting);
            if active.is_some() { calls.push(Call::Close(LINK)); }
            assert_eq!(*shared.calls.lock().unwrap(), calls);
        });
    }
}
