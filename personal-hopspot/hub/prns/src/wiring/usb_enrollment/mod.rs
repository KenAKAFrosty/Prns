use crate::{
    ControllerInstallation, DeviceStore, EnrollmentCoordinator, EnrollmentCoordinatorError,
    EnrollmentCoordinatorOutcome, NativeUsbDiscoveryEvent, NativeUsbDiscoveryRuntime,
};
use alloc::boxed::Box;
use core::future::Future;
use hopspot_hub_core::*;
use personal_rns::manifold::tokio::TokioClock;
use personal_rns::remote_control::{
    RemoteControlControllerIdentity, RemoteControlPairingAdmission,
    RemoteControlPairingAvailabilityKind,
};
use personal_rns::runtime::{
    ApproveRemoteControlControllerPairingControlError, InitiateRemoteControlControllerPairing,
    InitiateRemoteControlControllerPairingError, NodeRunError,
    RemoteControlControllerPairingInitiationControl,
    RemoteControlControllerPairingInitiationTransport,
};
use pipecircuit::StateMachine;
use tokio::task::JoinError;

#[cfg(test)]
mod behavior;
mod events;
#[cfg(test)]
mod tests;

use events::enrollment_events;
pub use events::{EnrollmentEvent, EnrollmentEvents, EnrollmentEventsFailure};

pub struct NativeUsbEnrollmentRuntime<const CAPACITY: usize, Run> {
    pub discovery: NativeUsbDiscoveryRuntime<CAPACITY, Run>,
    pub events: EnrollmentEvents,
}

pub fn prepare_native_usb_enrollment<const CAPACITY: usize>(
    installation: ControllerInstallation,
    on_event: impl FnMut(NativeUsbDiscoveryEvent<'_, CAPACITY>),
    shutdown: impl Future<Output = ()>,
) -> NativeUsbEnrollmentRuntime<CAPACITY, impl Future<Output = Result<(), NodeRunError>>> {
    let usb = personal_rns::prelude::AutoUsb::default();
    let rescan = usb.rescan_signal();
    prepare_enrollment(
        installation,
        move |handle| handle.attach(usb),
        rescan,
        on_event,
        shutdown,
    )
}

fn prepare_enrollment<const CAPACITY: usize>(
    installation: ControllerInstallation,
    attach: impl FnOnce(
        &personal_rns::prelude::PrnsNodeHandle,
    ) -> personal_rns::runtime::AttachedInterface,
    rescan: alloc::sync::Arc<tokio::sync::Notify>,
    mut on_event: impl FnMut(NativeUsbDiscoveryEvent<'_, CAPACITY>),
    shutdown: impl Future<Output = ()>,
) -> NativeUsbEnrollmentRuntime<CAPACITY, impl Future<Output = Result<(), NodeRunError>>> {
    let (sink, events) = enrollment_events();
    let discovery = super::usb_discovery::prepare_discovery(
        installation,
        attach,
        rescan,
        move |event| match event {
            NativeUsbDiscoveryEvent::Prns(event) => {
                if let Some(event) = sink.capture(event) {
                    on_event(NativeUsbDiscoveryEvent::Prns(event));
                }
            }
            event @ NativeUsbDiscoveryEvent::Discovery(_) => on_event(event),
        },
        shutdown,
    );
    NativeUsbEnrollmentRuntime { discovery, events }
}

pub struct UsbEnrollmentWork {
    pub registry: DeviceRegistry,
    pub store: DeviceStore,
    pub events: EnrollmentEvents,
    pub candidate: PairingCandidate,
    pub device: DeviceId,
    pub controller: RemoteControlControllerIdentity,
}

#[derive(Debug)]
pub enum UsbEnrollmentFailure {
    Clock(ReviewUsbEnrollmentOfferError),
    Begin(BeginEnrollmentError),
    Persistence(EnrollmentCoordinatorError),
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum UsbEnrollmentOutcome {
    InitiationFailed(Box<InitiateRemoteControlControllerPairingError>),
    ApprovalFailed(ApproveRemoteControlControllerPairingControlError),
    ApprovalNotAdvanced(personal_rns::engine::RemoteControlControllerPairingResponseReceived),
    EventsUnavailable { reason: EnrollmentEventsFailure },
    Ready { device: DeviceId },
    InvitationRequired,
    Expired,
    OfferRefused(ReviewUsbEnrollmentOfferOutcome),
    EnrollmentRefused(BeginEnrollmentOutcome),
    Settlement(EnrollmentCoordinatorOutcome),
}

pub struct UsbEnrollmentExit {
    pub registry: DeviceRegistry,
    pub store: DeviceStore,
    pub events: EnrollmentEvents,
    pub coordinator: Option<EnrollmentCoordinator>,
    pub cleanup: Option<personal_rns::runtime::RemoteControlPairingLinkCleanupOutcome>,
    pub outcome: Result<UsbEnrollmentOutcome, UsbEnrollmentFailure>,
}

impl UsbEnrollmentWork {
    pub async fn run<const CAPACITY: usize>(
        self,
        backend: impl RemoteControlControllerPairingInitiationTransport + Send + 'static,
        clock: TokioClock,
    ) -> Result<UsbEnrollmentExit, JoinError> {
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime.block_on(self.enroll::<CAPACITY>(backend, clock))
        })
        .await
    }

    async fn enroll<const CAPACITY: usize>(
        mut self,
        backend: impl RemoteControlControllerPairingInitiationTransport,
        clock: TokioClock,
    ) -> UsbEnrollmentExit {
        let mut coordinator = None;
        let mut pairing_link = None;
        let outcome = self
            .perform::<CAPACITY>(&backend, &clock, &mut coordinator, &mut pairing_link)
            .await;
        let cleanup = pairing_link.map(|link| backend.close_remote_control_pairing_link(link));
        UsbEnrollmentExit {
            registry: self.registry,
            store: self.store,
            events: self.events,
            coordinator,
            cleanup,
            outcome,
        }
    }

    async fn perform<const CAPACITY: usize>(
        &mut self,
        backend: &impl RemoteControlControllerPairingInitiationTransport,
        clock: &TokioClock,
        coordinator: &mut Option<EnrollmentCoordinator>,
        pairing_link: &mut Option<personal_rns::routing::links::LinkId>,
    ) -> Result<UsbEnrollmentOutcome, UsbEnrollmentFailure> {
        if self.candidate.kind() != RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable {
            return Ok(UsbEnrollmentOutcome::InvitationRequired);
        }
        if clock.now() >= self.candidate.expires_at() {
            return Ok(UsbEnrollmentOutcome::Expired);
        }
        if let Err(failure) = backend
            .initiate_remote_control_controller_pairing(InitiateRemoteControlControllerPairing {
                endpoint: self.candidate.endpoint(),
                admission: RemoteControlPairingAdmission::DirectPhysical,
                expires_at: self.candidate.expires_at(),
            })
            .await
        {
            return Ok(UsbEnrollmentOutcome::InitiationFailed(Box::new(failure)));
        }
        let mut approval = UsbEnrollmentApproval::new(self.candidate, self.device, self.controller);
        loop {
            let event = match self.events.receive().await {
                Ok(event) => event,
                Err(reason) => return Ok(UsbEnrollmentOutcome::EventsUnavailable { reason }),
            };
            match event {
                EnrollmentEvent::Confirmation(confirmation) if coordinator.is_none() => {
                    let offer = confirmation.confirmation();
                    *pairing_link = Some(offer.context().link_id());
                    let decision = approval
                        .step(ReviewUsbEnrollmentOffer {
                            now: clock.now(),
                            endpoint: offer.context().endpoint(),
                            controller: *offer.controller(),
                            target: RemoteControlTargetIdentity::new(*offer.target().public_keys()),
                            attempt: offer.attempt_id(),
                            authority: offer.permissions().authority(),
                            expires_at: confirmation.window().expires_at(),
                        })
                        .map_err(UsbEnrollmentFailure::Clock)?;
                    let ReviewUsbEnrollmentOfferOutcome::Approve { enrollment } = decision else {
                        return Ok(UsbEnrollmentOutcome::OfferRefused(decision));
                    };
                    let target = RemoteControlTargetIdentity::new(*enrollment.target.public_keys());
                    let begun = self
                        .registry
                        .step(enrollment)
                        .map_err(UsbEnrollmentFailure::Begin)?;
                    let BeginEnrollmentOutcome::Started { enrollment } = begun else {
                        return Ok(UsbEnrollmentOutcome::EnrollmentRefused(begun));
                    };
                    *coordinator = Some(EnrollmentCoordinator::new(CompleteEnrollment {
                        enrollment,
                        target,
                    }));
                    let response = match backend
                        .approve_remote_control_controller_pairing(confirmation.approval())
                        .await
                    {
                        Ok(response) => response,
                        Err(failure) => return Ok(UsbEnrollmentOutcome::ApprovalFailed(failure)),
                    };
                    if response.effect != personal_rns::engine::RemoteControlControllerPairingResponseEffect::Advanced {
                        return Ok(UsbEnrollmentOutcome::ApprovalNotAdvanced(response));
                    }
                }
                EnrollmentEvent::Confirmation(_) => {}
                EnrollmentEvent::Settlement(result) => {
                    let Some(coordinator) = coordinator.as_mut() else {
                        continue;
                    };
                    let outcome = coordinator
                        .observe::<CAPACITY>(result, &mut self.registry, &mut self.store)
                        .map_err(UsbEnrollmentFailure::Persistence)?;
                    match outcome {
                        EnrollmentCoordinatorOutcome::Recorded { enrollment } => {
                            return Ok(UsbEnrollmentOutcome::Ready {
                                device: enrollment.device(),
                            });
                        }
                        EnrollmentCoordinatorOutcome::Unhandled
                        | EnrollmentCoordinatorOutcome::UnrelatedAttempt { .. }
                        | EnrollmentCoordinatorOutcome::AwaitingAuthorization
                        | EnrollmentCoordinatorOutcome::AwaitingPersistence
                        | EnrollmentCoordinatorOutcome::Settled => {}
                        outcome @ (EnrollmentCoordinatorOutcome::Failed { .. }
                        | EnrollmentCoordinatorOutcome::MissingDevice { .. }
                        | EnrollmentCoordinatorOutcome::StaleEnrollment { .. }
                        | EnrollmentCoordinatorOutcome::TargetMismatch { .. }
                        | EnrollmentCoordinatorOutcome::TargetAlreadyPaired { .. }
                        | EnrollmentCoordinatorOutcome::InsufficientCapacity {
                            ..
                        }) => return Ok(UsbEnrollmentOutcome::Settlement(outcome)),
                    }
                }
            }
        }
    }
}

use personal_rns::remote_control::RemoteControlTargetIdentity;
