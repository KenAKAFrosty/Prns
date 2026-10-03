#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::tests::*;
use crate::{DeviceSessionMessage, MioSessionSubmission, PrnsDeviceOut};
use pipecircuit::{ConductionFailure, Pipecircuit};

#[test]
fn conduction_preserves_each_route_and_recovers_the_registry_on_mailbox_close() {
    let (registry, connection, _) = fixture();
    let (mut reference, _, _) = fixture();
    let mut reference_board = DeviceSessionSwitchboard::<4>::new(connection.device());
    let (reactor, sender) = MioSessionReactor::try_open().unwrap();
    let mut conductor = Pipecircuit::new(
        DeviceSessionSwitchboard::<4>::new(connection.device()),
        BoundedCircuit::new(MioDeviceSessionCircuit::new(registry, reactor), 7),
    );
    for make_message in [
        || DeviceSessionMessage::Inspect,
        || {
            DeviceSessionMessage::Prns(PrnsDeviceOut::Connected {
                confirmation: ConfirmConnection {
                    connection: fixture().1,
                    target: fixture().1.target(),
                    link: LINK,
                },
            })
        },
        || DeviceSessionMessage::Refresh,
        || DeviceSessionMessage::Disconnect,
        || DeviceSessionMessage::Connect,
        || DeviceSessionMessage::Inspect,
    ] {
        let expected = reference_board
            .route(DeviceSessionInput {
                registry: &mut reference,
                message: make_message(),
            })
            .unwrap();
        assert!(matches!(
            sender.submit(make_message()).unwrap(),
            MioSessionSubmission::Submitted
        ));
        assert_eq!(
            conductor.conduct().unwrap(),
            MioDeviceSessionTurn::Routed(expected)
        );
    }
    drop(sender);
    assert_eq!(
        conductor.conduct().unwrap(),
        MioDeviceSessionTurn::SendersClosed
    );
    let (_, circuit) = conductor.into_parts();
    assert_eq!(
        circuit.circuit.into_registry().step(ReadDevice {
            device: connection.device()
        }),
        reference.step(ReadDevice {
            device: connection.device()
        })
    );
}

#[test]
fn stale_readiness_does_not_route_and_foreign_messages_surface_the_exact_invariant() {
    let (mut registry, connection, _) = fixture();
    let foreign_device = paired(&mut registry, target(99));
    let foreign = begin(&mut registry, foreign_device);
    let (reactor, sender) = MioSessionReactor::try_open().unwrap();
    let mut circuit = MioDeviceSessionCircuit::new(registry, reactor);
    let mut board = DeviceSessionSwitchboard::<4>::new(connection.device());
    assert!(matches!(
        circuit
            .react(&mut board, &MioSessionReaction::MessageReady)
            .unwrap(),
        CircuitReactionOutcome::AwaitingReaction
    ));
    assert!(matches!(
        sender
            .submit(DeviceSessionMessage::Prns(PrnsDeviceOut::StaleClose {
                connection: foreign
            }))
            .unwrap(),
        MioSessionSubmission::Submitted
    ));
    let mut conductor = Pipecircuit::new(board, BoundedCircuit::new(circuit, 2));
    assert!(
        matches!(conductor.conduct(), Err(ConductionFailure::Circuit { failure: DeviceSessionRoutingError::WrongDevice { expected, received } }) if expected == connection.device() && received == foreign_device)
    );
    assert!(matches!(
        sender.submit(DeviceSessionMessage::Inspect).unwrap(),
        MioSessionSubmission::Submitted
    ));
    let MioDeviceSessionTurn::Routed(route) = conductor.conduct().unwrap() else {
        panic!("mailbox closed")
    };
    let (_, circuit) = conductor.into_parts();
    let mut registry = circuit.circuit.into_registry();
    assert_eq!(
        route.snapshot.device,
        registry.step(ReadDevice {
            device: connection.device()
        })
    );
    assert!(matches!(
        registry.step(ReadDevice {
            device: foreign_device
        }),
        ReadDeviceOutcome::Found { .. }
    ));
}
