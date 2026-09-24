use super::*;
use adapter::Handle;
use personal_rns::engine::{EgressTarget, OpenRemoteControlPairing, RemoteControlPairingOpened};
use personal_rns::runtime::{
    InitiateRemoteControlControllerPairing, Message, PrnsEvent,
    RemoteControlControllerPairingConfirmation, RemoteControlControllerPairingInitiationControl,
    RemoteControlPairingControl, RemoteControlTargetPairingConfirmation,
};
use personal_rns::units::DurationMillis;

#[derive(Debug, PartialEq, Eq)]
pub enum Observation {
    Available {
        node: usize,
        endpoint: RemoteControlPairingEndpoint,
        expires_at: personal_rns::units::InstantMillis,
    },
    Controller(RemoteControlControllerPairingConfirmation),
    Target(RemoteControlTargetPairingConfirmation),
    ControllerPersisted,
    TargetPersisted,
}
pub fn observe(sink: &Rc<RefCell<Vec<Observation>>>, node: usize, event: PrnsEvent<'_>) {
    let observation = match event {
        PrnsEvent::Message(Message::RemoteControlPairingAvailable(available)) => {
            Observation::Available {
                node,
                endpoint: available.endpoint(),
                expires_at: available.expires_at(),
            }
        }
        PrnsEvent::Message(Message::RemoteControlControllerPairingConfirmationRequired(
            confirmation,
        )) => Observation::Controller(confirmation),
        PrnsEvent::Message(Message::RemoteControlTargetPairingConfirmationRequired(
            confirmation,
        )) => Observation::Target(confirmation),
        PrnsEvent::Message(Message::RemoteControlControllerPairingAuthorizationPersisted {
            ..
        }) => Observation::ControllerPersisted,
        PrnsEvent::Message(Message::RemoteControlTargetPairingAuthorizationPersisted {
            ..
        }) => Observation::TargetPersisted,
        _ => return,
    };
    let mut observations = sink.borrow_mut();
    assert!(observations.len() < 512, "bounded pairing observations");
    observations.push(observation);
}
impl Handle {
    pub async fn open_pairing(&self) -> RemoteControlPairingOpened {
        let open = OpenRemoteControlPairing {
            target: EgressTarget::AllInterfaces,
            expires_after: RemoteControlPairingExpiresAfter::try_from(DurationMillis(30_000))
                .expect("pairing window"),
            attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(DurationMillis(10_000))
                .expect("attempt"),
            permissions: RemoteControlPairingPermissions::new(
                RemoteControlControllerAuthority::Administrator,
                requests(),
            )
            .expect("requested authority"),
            public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
                &b"qualification"[..],
            )
            .expect("metadata"),
        };
        match self {
            Handle::Tokio(h) => h
                .open_remote_control_pairing(open)
                .await
                .expect("open pairing"),
            Handle::Embassy(h) => h
                .open_remote_control_pairing(open)
                .await
                .expect("open pairing"),
        }
    }
    pub async fn initiate(&self, opened: RemoteControlPairingOpened) {
        let input = InitiateRemoteControlControllerPairing {
            endpoint: opened.endpoint,
            invitation_code: opened.invitation_code,
            expires_at: opened.expires_at,
        };
        match self {
            Handle::Tokio(h) => {
                h.initiate_remote_control_controller_pairing(input)
                    .await
                    .expect("controller starts pairing");
            }
            Handle::Embassy(h) => {
                h.initiate_remote_control_controller_pairing(input)
                    .await
                    .expect("controller starts pairing");
            }
        }
    }
    pub async fn approve_target(&self, confirmation: RemoteControlTargetPairingConfirmation) {
        match self {
            Handle::Tokio(h) => {
                h.approve_remote_control_target_pairing(confirmation.approval())
                    .await
                    .expect("target approves");
            }
            Handle::Embassy(h) => {
                h.approve_remote_control_target_pairing(confirmation.approval())
                    .await
                    .expect("target approves");
            }
        }
    }
    pub async fn approve_controller(
        &self,
        confirmation: RemoteControlControllerPairingConfirmation,
    ) {
        match self {
            Handle::Tokio(h) => {
                h.approve_remote_control_controller_pairing(confirmation.approval())
                    .await
                    .expect("controller approves");
            }
            Handle::Embassy(h) => {
                h.approve_remote_control_controller_pairing(confirmation.approval())
                    .await
                    .expect("controller approves");
            }
        }
    }
}

impl Handle {
    pub async fn close_pairing(&self) {
        match self {
            Handle::Tokio(h) => {
                h.close_remote_control_pairing()
                    .await
                    .expect("close pairing window");
            }
            Handle::Embassy(h) => {
                h.close_remote_control_pairing()
                    .await
                    .expect("close pairing window");
            }
        }
    }
    pub async fn reject_controller(
        &self,
        confirmation: RemoteControlControllerPairingConfirmation,
    ) {
        match self {
            Handle::Tokio(h) => {
                h.reject_remote_control_controller_pairing(confirmation.rejection())
                    .await
                    .expect("reject controller");
            }
            Handle::Embassy(h) => {
                h.reject_remote_control_controller_pairing(confirmation.rejection())
                    .await
                    .expect("reject controller");
            }
        }
    }
    pub async fn reject_target(&self, confirmation: RemoteControlTargetPairingConfirmation) {
        match self {
            Handle::Tokio(h) => {
                h.reject_remote_control_target_pairing(confirmation.rejection())
                    .await
                    .expect("reject target");
            }
            Handle::Embassy(h) => {
                h.reject_remote_control_target_pairing(confirmation.rejection())
                    .await
                    .expect("reject target");
            }
        }
    }
}

pub fn observed_offer(
    sink: &Rc<RefCell<Vec<Observation>>>,
    opened: RemoteControlPairingOpened,
) -> RemoteControlPairingOpened {
    observed_offer_for(sink, CONTROLLER, opened)
}

pub fn observed_offer_for(
    sink: &Rc<RefCell<Vec<Observation>>>,
    controller: usize,
    mut opened: RemoteControlPairingOpened,
) -> RemoteControlPairingOpened {
    let mut events = sink.borrow_mut();
    let index = events.iter().position(|event| matches!(event, Observation::Available { node, endpoint, .. } if *node == controller && *endpoint == opened.endpoint)).expect("controller-local availability deadline");
    let Observation::Available { expires_at, .. } = events.remove(index) else {
        unreachable!("availability")
    };
    opened.expires_at = expires_at;
    opened
}
