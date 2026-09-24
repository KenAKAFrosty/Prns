use std::cell::RefCell;
use std::rc::Rc;

use personal_rns::engine::RemoteControlPairingOpened;
use personal_rns::remote_control::{RemoteControlPairingAttemptId, RemoteControlPairingEndpoint};
use personal_rns::runtime::{
    Message, PrnsEvent, RemoteControlControllerPairingConfirmation,
    RemoteControlTargetPairingConfirmation,
};
use personal_rns::units::InstantMillis;

const MAX_PAIRING_OBSERVATIONS: usize = 512;

#[derive(Debug, PartialEq, Eq)]
pub enum PairingEvent {
    Available {
        endpoint: RemoteControlPairingEndpoint,
        expires_at: InstantMillis,
    },
    ControllerConfirmation(RemoteControlControllerPairingConfirmation),
    TargetConfirmation(RemoteControlTargetPairingConfirmation),
    ControllerPersisted(RemoteControlPairingAttemptId),
    TargetPersisted(RemoteControlPairingAttemptId),
    ControllerPersistenceFailed(RemoteControlPairingAttemptId),
    TargetExpiredDuringAuthorization(RemoteControlPairingAttemptId),
    Expired,
    LinkClosed,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PairingObservation {
    pub node: usize,
    pub generation: u64,
    pub event: PairingEvent,
}

pub(super) fn observe(
    observations: &Rc<RefCell<Vec<PairingObservation>>>,
    node: usize,
    generation: u64,
    event: PrnsEvent<'_>,
) {
    let event = match event {
        PrnsEvent::Message(Message::RemoteControlPairingAvailable(available)) => {
            PairingEvent::Available {
                endpoint: available.endpoint(),
                expires_at: available.expires_at(),
            }
        }
        PrnsEvent::Message(Message::RemoteControlControllerPairingConfirmationRequired(
            confirmation,
        )) => PairingEvent::ControllerConfirmation(confirmation),
        PrnsEvent::Message(Message::RemoteControlTargetPairingConfirmationRequired(
            confirmation,
        )) => PairingEvent::TargetConfirmation(confirmation),
        PrnsEvent::Message(Message::RemoteControlControllerPairingAuthorizationPersisted {
            attempt_id,
        }) => PairingEvent::ControllerPersisted(attempt_id),
        PrnsEvent::Message(Message::RemoteControlTargetPairingAuthorizationPersisted {
            attempt_id,
            ..
        }) => PairingEvent::TargetPersisted(attempt_id),
        PrnsEvent::Message(
            Message::RemoteControlControllerPairingAuthorizationPersistenceFailed { attempt_id },
        ) => PairingEvent::ControllerPersistenceFailed(attempt_id),
        PrnsEvent::Message(Message::RemoteControlTargetPairingExpiredDuringAuthorization {
            attempt_id,
        }) => PairingEvent::TargetExpiredDuringAuthorization(attempt_id),
        PrnsEvent::Message(
            Message::RemoteControlControllerPairingExpired { .. }
            | Message::RemoteControlTargetPairingExpired { .. },
        ) => PairingEvent::Expired,
        PrnsEvent::Message(
            Message::RemoteControlControllerPairingLinkClosed { .. }
            | Message::RemoteControlTargetPairingLinkClosed { .. },
        ) => PairingEvent::LinkClosed,
        _ => return,
    };
    let mut observations = observations.borrow_mut();
    assert!(
        observations.len() < MAX_PAIRING_OBSERVATIONS,
        "bounded pairing observations"
    );
    observations.push(PairingObservation {
        node,
        generation,
        event,
    });
}

impl super::Lab<'_> {
    pub fn open_pairing(&mut self) -> RemoteControlPairingOpened {
        let handle = self.nodes[super::TARGET].handle.clone();
        let task = self.insert(async move {
            let opened = handle.open_remote_control_pairing(personal_rns::engine::OpenRemoteControlPairing {
                target: personal_rns::engine::EgressTarget::AllInterfaces,
                expires_after: personal_rns::remote_control::RemoteControlPairingExpiresAfter::try_from(personal_rns::units::DurationMillis(30_000)).expect("window"),
                attempt_timeout: personal_rns::remote_control::RemoteControlPairingAttemptTimeout::try_from(personal_rns::units::DurationMillis(10_000)).expect("attempt"),
                permissions: personal_rns::remote_control::RemoteControlPairingPermissions::new(personal_rns::remote_control::RemoteControlControllerAuthority::Operator, super::requests()).expect("pairing permissions"),
                public_app_data: personal_rns::remote_control::RemoteControlPairingPublicAppDataBytes::try_from(&b"simulation"[..]).expect("bounded metadata"),
            }).await.expect("open pairing");
            super::Event::PairingOpened(opened)
        });
        let mut completed = self.settle();
        assert_eq!(completed.len(), 1);
        let (found, super::Event::PairingOpened(opened)) = completed.remove(0) else {
            unreachable!("pairing window");
        };
        assert_eq!(found, task);
        opened
    }

    pub fn controller_confirmation(
        &self,
        node: usize,
    ) -> RemoteControlControllerPairingConfirmation {
        let mut observations = self.pairing.borrow_mut();
        let index = observations
            .iter()
            .position(|observation| {
                observation.node == node
                    && matches!(observation.event, PairingEvent::ControllerConfirmation(_))
            })
            .expect("controller confirmation reached application callback");
        let PairingEvent::ControllerConfirmation(confirmation) = observations.remove(index).event
        else {
            unreachable!("controller confirmation");
        };
        confirmation
    }

    pub fn target_confirmation(&self) -> RemoteControlTargetPairingConfirmation {
        let mut observations = self.pairing.borrow_mut();
        let index = observations
            .iter()
            .position(|observation| {
                observation.node == super::TARGET
                    && matches!(observation.event, PairingEvent::TargetConfirmation(_))
            })
            .expect("target confirmation reached application callback");
        let PairingEvent::TargetConfirmation(confirmation) = observations.remove(index).event
        else {
            unreachable!("target confirmation");
        };
        confirmation
    }
}
