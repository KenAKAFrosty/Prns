use crate::crypto::ratchets::RatchetPolicy;
use crate::engine::node_egress::fan_frame;
use crate::engine::{
    CloseRemoteControlPairingFailure, CloseRemoteControlPairingOutcome, CommandId, CommandOutcome,
    EgressTarget, EngineReaction, FanTarget, OpenRemoteControlPairing,
    OpenRemoteControlPairingFailure, OpenRemoteControlPairingRejection, RemoteControlPairingOpened,
    RetireDestinationOutcome, SendPlainPacket, SendPlainPacketPayload,
};
use crate::identity::held::ReleaseHeldIdentityOutcome;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::vault::IdentitySecretKey;
use crate::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::AttachedInterfaces;
use crate::remote_control::{
    CloseRemoteControlPairingOutcome as PairingStateCloseOutcome,
    OpenRemoteControlPairingOutcome as PairingStateOpenOutcome, RemoteControlPairingAvailability,
    RemoteControlPairingAvailabilityDestination, RemoteControlPairingAvailabilityDestinationError,
    RemoteControlPairingIdentity, RemoteControlPairingSession, RemoteControlPairingState,
    RemoteControlPairingView, RemoteControlPairingWindow,
    REMOTE_CONTROL_PAIRING_APPLICATION_ASPECTS, REMOTE_CONTROL_PAIRING_APPLICATION_NAME,
};
use crate::routing::announce::{AnnounceEntropy, AnnounceId};
use crate::routing::{LinkRequestPolicy, ProofStrategy};
use crate::storage::StorageLayout;
use crate::units::InstantMillis;
use crate::wire::{BROADCAST_MDU, BROADCAST_MTU};

const PAIRING_IDENTITY_MINT_ATTEMPTS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureRemoteControlPairingError {
    AlreadyConfigured,
    RegisterAvailability(crate::routing::upstream_app_destinations::RegisterDestinationError),
    AvailabilityDestination(RemoteControlPairingAvailabilityDestinationError),
}

impl<S: StorageLayout> crate::engine::EngineState<S> {
    pub fn configure_remote_control_pairing(
        &mut self,
    ) -> Result<RemoteControlPairingAvailabilityDestination, ConfigureRemoteControlPairingError>
    {
        match self.remote_control_pairing.view() {
            RemoteControlPairingView::Unavailable => {}
            RemoteControlPairingView::Closed | RemoteControlPairingView::Open(_) => {
                return Err(ConfigureRemoteControlPairingError::AlreadyConfigured)
            }
        }
        let destination = self
            .register_plain_destination(
                REMOTE_CONTROL_PAIRING_APPLICATION_NAME,
                REMOTE_CONTROL_PAIRING_APPLICATION_ASPECTS,
            )
            .map_err(ConfigureRemoteControlPairingError::RegisterAvailability)?;
        let destination = RemoteControlPairingAvailabilityDestination::try_from(destination)
            .map_err(ConfigureRemoteControlPairingError::AvailabilityDestination)?;
        self.remote_control_pairing = RemoteControlPairingState::available();
        Ok(destination)
    }

    #[must_use]
    pub const fn remote_control_pairing_view(&self) -> RemoteControlPairingView<'_> {
        self.remote_control_pairing.view()
    }

    pub(crate) fn ingest_open_remote_control_pairing(
        &self,
        id: CommandId,
        open: OpenRemoteControlPairing,
        interfaces: AttachedInterfaces<'_>,
    ) -> CommandOutcome {
        let rejection = match self.remote_control_pairing.view() {
            RemoteControlPairingView::Unavailable => {
                Some(OpenRemoteControlPairingRejection::Unavailable)
            }
            RemoteControlPairingView::Open(_) => {
                Some(OpenRemoteControlPairingRejection::AlreadyOpen)
            }
            RemoteControlPairingView::Closed => match open.target {
                EgressTarget::AllInterfaces
                    if !interfaces
                        .iter()
                        .any(|interface| interface.capabilities.allows_transmit()) =>
                {
                    Some(OpenRemoteControlPairingRejection::NoTransmittingInterfaces)
                }
                target => target
                    .admit(interfaces)
                    .err()
                    .map(OpenRemoteControlPairingRejection::EgressTarget),
            },
        };
        match rejection {
            Some(rejection) => CommandOutcome::OpenRemoteControlPairingRejected { id, rejection },
            None => CommandOutcome::OwesOpenRemoteControlPairing { id, open },
        }
    }

    pub(crate) fn ingest_close_remote_control_pairing(&self, id: CommandId) -> CommandOutcome {
        match self.remote_control_pairing.view() {
            RemoteControlPairingView::Unavailable => {
                CommandOutcome::CloseRemoteControlPairingRejected {
                    id,
                    failure: CloseRemoteControlPairingFailure::Unavailable,
                }
            }
            RemoteControlPairingView::Closed | RemoteControlPairingView::Open(_) => {
                CommandOutcome::OwesCloseRemoteControlPairing { id }
            }
        }
    }

    pub(crate) fn open_remote_control_pairing_into<F>(
        &mut self,
        open: OpenRemoteControlPairing,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> Result<RemoteControlPairingOpened, OpenRemoteControlPairingFailure>
    where
        F: FnMut(&mut [u8]),
    {
        let expires_at = open.expires_after.deadline_from(now);
        let window = RemoteControlPairingWindow::new(now, expires_at).map_err(|_| {
            OpenRemoteControlPairingFailure::Rejected(
                OpenRemoteControlPairingRejection::DeadlineOverflow,
            )
        })?;
        let (identity_secret, signer) = self.mint_pairing_identity(fill_entropy)?;
        let identity_hash = signer.identity_hash();
        let identity = RemoteControlPairingIdentity::new(identity_hash);
        let endpoint = identity.endpoint();

        let mut announce_entropy = [0u8; AnnounceEntropy::LEN];
        fill_entropy(&mut announce_entropy);
        let announce_id = AnnounceId::mint(AnnounceEntropy::new(announce_entropy), now);
        let mut availability = [0u8; BROADCAST_MDU];
        let availability_len = RemoteControlPairingAvailability::write_signed(
            &signer,
            announce_id,
            open.expires_after,
            open.public_app_data.as_borrowed(),
            &mut availability,
        )
        .map_err(OpenRemoteControlPairingFailure::WriteAvailability)?;
        let payload = SendPlainPacketPayload::from_slice(&availability[..availability_len])
            .map_err(|()| OpenRemoteControlPairingFailure::PayloadCapacity)?;
        let send = SendPlainPacket {
            destination: RemoteControlPairingAvailabilityDestination::canonical()
                .destination_hash(),
            target: open.target,
            payload,
        };
        let mut frame = [0u8; BROADCAST_MTU];
        let frame_len = self
            .write_commanded_send_plain_packet(&send, &mut frame)
            .map_err(OpenRemoteControlPairingFailure::WritePacket)?;

        self.hold_identity(identity_secret)
            .map_err(OpenRemoteControlPairingFailure::HoldIdentity)?;
        if let Err(error) = self.register_single_destination(
            &identity_hash,
            REMOTE_CONTROL_PAIRING_APPLICATION_NAME,
            REMOTE_CONTROL_PAIRING_APPLICATION_ASPECTS,
            b"",
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptDirect,
            RatchetPolicy::NoRatchets,
        ) {
            let _released = self.held_identities.release(&identity_hash);
            return Err(OpenRemoteControlPairingFailure::RegisterEndpoint(error));
        }

        let session = RemoteControlPairingSession::new(identity, window, open.permissions);
        let rejected = match self.remote_control_pairing.open(session, now) {
            PairingStateOpenOutcome::Opened => None,
            PairingStateOpenOutcome::Unavailable { unopened } => {
                Some((unopened, OpenRemoteControlPairingRejection::Unavailable))
            }
            PairingStateOpenOutcome::AlreadyOpen { unopened } => {
                Some((unopened, OpenRemoteControlPairingRejection::AlreadyOpen))
            }
            PairingStateOpenOutcome::DeadlineElapsed { unopened } => Some((
                unopened,
                OpenRemoteControlPairingRejection::DeadlineOverflow,
            )),
        };
        if let Some((unopened, rejection)) = rejected {
            let endpoint = unopened.endpoint();
            let _unregistered = self.unregister_destination(&endpoint.destination_hash());
            let _released = self
                .held_identities
                .release(&unopened.identity().identity_hash());
            return Err(OpenRemoteControlPairingFailure::Rejected(rejection));
        }

        let fanout = match send.target {
            EgressTarget::AllInterfaces => FanTarget::All,
            EgressTarget::Interface(interface) => FanTarget::Only(interface),
        };
        fan_frame(interfaces, fanout, &frame[..frame_len], sink);
        Ok(RemoteControlPairingOpened {
            endpoint,
            expires_at,
        })
    }

    pub(crate) fn close_remote_control_pairing_into<F>(
        &mut self,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> Result<CloseRemoteControlPairingOutcome, CloseRemoteControlPairingFailure>
    where
        F: FnMut(&mut [u8]),
    {
        let (endpoint, identity) = match self.remote_control_pairing.view() {
            RemoteControlPairingView::Unavailable => {
                return Err(CloseRemoteControlPairingFailure::Unavailable)
            }
            RemoteControlPairingView::Closed => {
                return Ok(CloseRemoteControlPairingOutcome::AlreadyClosed)
            }
            RemoteControlPairingView::Open(session) => {
                (session.endpoint(), session.identity().identity_hash())
            }
        };

        match self.retire_destination(&endpoint.destination_hash(), interfaces, fill_entropy, sink)
        {
            RetireDestinationOutcome::Retired { .. } => {}
            RetireDestinationOutcome::RetirementIncomplete {
                first_remaining_link,
                retired_links,
            } => {
                return Err(CloseRemoteControlPairingFailure::RetirementIncomplete {
                    first_remaining_link,
                    retired_links,
                })
            }
            RetireDestinationOutcome::NotRegistered => {
                let _closed = self.remote_control_pairing.close();
                let _released = self.held_identities.release(&identity);
                return Err(CloseRemoteControlPairingFailure::EndpointNotRegistered);
            }
        }
        let release = self.held_identities.release(&identity);
        let closed = self.remote_control_pairing.close();
        match (release, closed) {
            (ReleaseHeldIdentityOutcome::Released, PairingStateCloseOutcome::Closed { .. }) => {
                Ok(CloseRemoteControlPairingOutcome::Closed { endpoint })
            }
            (ReleaseHeldIdentityOutcome::NotHeld, _) => {
                Err(CloseRemoteControlPairingFailure::IdentityNotHeld)
            }
            (
                ReleaseHeldIdentityOutcome::Released,
                PairingStateCloseOutcome::AlreadyClosed | PairingStateCloseOutcome::Unavailable,
            ) => Err(CloseRemoteControlPairingFailure::EndpointNotRegistered),
        }
    }

    fn mint_pairing_identity<F>(
        &self,
        fill_entropy: &mut F,
    ) -> Result<(IdentitySecretKey, InMemoryNodeIdentity), OpenRemoteControlPairingFailure>
    where
        F: FnMut(&mut [u8]),
    {
        for _ in 0..PAIRING_IDENTITY_MINT_ATTEMPTS {
            let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
            fill_entropy(&mut secret[..]);
            let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
            if !self.held_identities.contains(&signer.identity_hash()) {
                return Ok((secret, signer));
            }
        }
        Err(OpenRemoteControlPairingFailure::IdentityGenerationExhausted)
    }

    pub fn fire_due_remote_control_pairing<F>(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let endpoint = match self.remote_control_pairing.view() {
            RemoteControlPairingView::Open(session) if now >= session.window().expires_at() => {
                session.endpoint()
            }
            RemoteControlPairingView::Unavailable
            | RemoteControlPairingView::Closed
            | RemoteControlPairingView::Open(_) => {
                return crate::engine::WakeSchedules {
                    remote_control_pairing: self.remote_control_pairing_wake(),
                    ..crate::engine::WakeSchedules::UNCHANGED
                }
            }
        };
        match self.close_remote_control_pairing_into(interfaces, fill_entropy, sink) {
            Ok(CloseRemoteControlPairingOutcome::Closed { .. }) => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlPairingExpired { endpoint },
                ));
            }
            Ok(CloseRemoteControlPairingOutcome::AlreadyClosed) => {}
            Err(failure) => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlPairingExpiryFailed {
                        endpoint,
                        failure,
                    },
                ));
            }
        }
        crate::engine::WakeSchedules {
            remote_control_pairing: self.remote_control_pairing_wake(),
            ..crate::engine::WakeSchedules::UNCHANGED
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use super::*;
    use crate::engine::test_support::{fixed_secret_key, routable_descriptor, TestStorageLayout};
    use crate::engine::{
        CloseRemoteControlPairing, CommandId, Directive, IssuedCommand, Journaled, PrnsCommand,
        Settlement, WakeReason,
    };
    use crate::interfaces::{EgressCapability, InterfaceId};
    use crate::remote_control::{
        RemoteControlPairingExpiresAfter, RemoteControlPairingPermissions,
        RemoteControlPairingPublicAppDataBytes, RemoteControlRequestKind, RemoteControlRequestSet,
    };
    use crate::routing::UpstreamAppDestinationKind;
    use crate::storage::TestFixedStorage;
    use crate::units::DurationMillis;
    use crate::wire::{DestinationType, PacketType, WirePacketHeader};

    fn open(target: EgressTarget) -> OpenRemoteControlPairing {
        OpenRemoteControlPairing {
            target,
            expires_after: RemoteControlPairingExpiresAfter::try_from(DurationMillis(60_000))
                .unwrap(),
            permissions: RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
                RemoteControlRequestKind::Describe,
            ))
            .unwrap(),
            public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
                b"nearby node".as_slice(),
            )
            .unwrap(),
        }
    }

    fn fill_pairing_entropy(bytes: &mut [u8]) {
        match bytes.len() {
            IDENTITY_SECRET_KEY_LEN => bytes.fill(0xA1),
            AnnounceEntropy::LEN => bytes.fill(0xB2),
            length => panic!("unexpected entropy request of {length} bytes"),
        }
    }

    #[test]
    fn configuration_registers_the_listener_before_becoming_available() {
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        let canonical = RemoteControlPairingAvailabilityDestination::canonical();
        assert_eq!(engine.configure_remote_control_pairing(), Ok(canonical));
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
        assert!(engine
            .upstream_app_destinations()
            .any(|registration| registration.destination == canonical.destination_hash()));
        assert_eq!(
            engine.configure_remote_control_pairing(),
            Err(ConfigureRemoteControlPairingError::AlreadyConfigured),
        );
        assert_eq!(engine.upstream_app_destinations().count(), 1);

        type FullRegistry = TestFixedStorage<8, 8, 512, 1, 2, 16, 2, 2, 2, 2, 4, 2>;
        let mut full = crate::engine::EngineState::<FullRegistry>::default();
        full.register_plain_destination("full", &["registry"])
            .unwrap();
        assert_eq!(
            full.configure_remote_control_pairing(),
            Err(ConfigureRemoteControlPairingError::RegisterAvailability(
                crate::routing::upstream_app_destinations::RegisterDestinationError::RegistryFull,
            )),
        );
        assert_eq!(
            full.remote_control_pairing_view(),
            RemoteControlPairingView::Unavailable,
        );
    }

    #[test]
    fn opening_emits_one_signed_availability_and_commits_the_exact_session() {
        let selected = InterfaceId::new([0x91; 8]);
        let other = InterfaceId::new([0x92; 8]);
        let interfaces = [routable_descriptor(other), routable_descriptor(selected)];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        assert_eq!(
            engine.configure_remote_control_pairing(),
            Ok(RemoteControlPairingAvailabilityDestination::canonical()),
        );
        let mut emitted = std::vec::Vec::new();
        let mut settled = None;

        let schedules = engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(41),
                command: PrnsCommand::OpenRemoteControlPairing(open(EgressTarget::Interface(
                    selected,
                ))),
            },
            AttachedInterfaces::new(&interfaces),
            InstantMillis(1_000),
            &mut fill_pairing_entropy,
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { target, bytes }) => {
                    emitted.push((target, bytes.to_vec()));
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled = Some((id, settlement));
                }
                EngineReaction::Directive(_) | EngineReaction::Journaled(_) => {}
            },
        );

        let [(target, frame)] = emitted.as_slice() else {
            panic!("one direct PLAIN frame")
        };
        assert_eq!(*target, selected);
        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.destination_type, DestinationType::Plain);
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(
            header.address,
            RemoteControlPairingAvailabilityDestination::canonical()
                .destination_hash()
                .to_address(),
        );
        let availability = RemoteControlPairingAvailability::parse(payload).unwrap();
        assert_eq!(availability.public_app_data().as_bytes(), b"nearby node");
        assert_eq!(
            availability.expires_after().duration(),
            DurationMillis(60_000),
        );
        let endpoint = availability.pairing_endpoint();
        assert_eq!(
            settled,
            Some((
                CommandId(41),
                Settlement::OpenRemoteControlPairing(Ok(RemoteControlPairingOpened {
                    endpoint,
                    expires_at: InstantMillis(61_000),
                })),
            )),
        );
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Open(&RemoteControlPairingSession::new(
                availability.pairing_identity(),
                RemoteControlPairingWindow::new(InstantMillis(1_000), InstantMillis(61_000),)
                    .unwrap(),
                RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
                    RemoteControlRequestKind::Describe,
                ))
                .unwrap(),
            )),
        );
        let endpoint_registration = engine
            .upstream_app_destinations()
            .find(|registration| registration.destination == endpoint.destination_hash())
            .unwrap();
        let UpstreamAppDestinationKind::Single {
            link_request_policy,
            ..
        } = endpoint_registration.kind
        else {
            panic!("the pairing endpoint must be a Single destination")
        };
        assert_eq!(link_request_policy, LinkRequestPolicy::AcceptDirect);
        assert_eq!(
            engine.held_identity_hashes(),
            &[availability.pairing_identity().identity_hash()]
        );
        assert_eq!(
            schedules.remote_control_pairing,
            crate::engine::WakeSchedule::At(InstantMillis(61_000)),
        );
        assert_eq!(
            engine.next_wake(InstantMillis(1_000), AttachedInterfaces::new(&interfaces),),
            crate::engine::NextWake::At {
                at: InstantMillis(61_000),
                reason: WakeReason::RemoteControlPairing,
            },
        );
    }

    #[test]
    fn unavailable_and_non_transmitting_opens_reject_before_drawing_entropy() {
        let receive_only = InterfaceId::new([0x93; 8]);
        let mut descriptor = routable_descriptor(receive_only);
        descriptor.capabilities.egress = EgressCapability::Disabled;
        let interfaces = [descriptor];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        let mut entropy_calls = 0usize;
        let mut settlements = std::vec::Vec::new();
        for (id, expected) in [
            (
                CommandId(51),
                OpenRemoteControlPairingRejection::Unavailable,
            ),
            (
                CommandId(52),
                OpenRemoteControlPairingRejection::NoTransmittingInterfaces,
            ),
        ] {
            if id == CommandId(52) {
                assert_eq!(
                    engine.configure_remote_control_pairing(),
                    Ok(RemoteControlPairingAvailabilityDestination::canonical()),
                );
            }
            let _ = engine.ingest_command_into(
                IssuedCommand {
                    id,
                    command: PrnsCommand::OpenRemoteControlPairing(open(
                        EgressTarget::AllInterfaces,
                    )),
                },
                AttachedInterfaces::new(&interfaces),
                InstantMillis(1_000),
                &mut |_| entropy_calls += 1,
                &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                        reaction
                    {
                        settlements.push((id, settlement));
                    }
                },
            );
            assert_eq!(
                settlements.last(),
                Some(&(
                    id,
                    Settlement::OpenRemoteControlPairing(Err(
                        OpenRemoteControlPairingFailure::Rejected(expected),
                    )),
                )),
            );
        }
        assert_eq!(entropy_calls, 0);
        assert_eq!(engine.held_identity_hashes(), &[]);
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
    }

    #[test]
    fn identity_minting_is_bounded_when_entropy_repeats_an_existing_identity() {
        let interface = InterfaceId::new([0x94; 8]);
        let interfaces = [routable_descriptor(interface)];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        let existing = engine.hold_identity(fixed_secret_key()).unwrap();
        assert_eq!(
            engine.configure_remote_control_pairing(),
            Ok(RemoteControlPairingAvailabilityDestination::canonical()),
        );
        let mut calls = 0usize;
        let mut settlement = None;
        let _ = engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(61),
                command: PrnsCommand::OpenRemoteControlPairing(open(EgressTarget::AllInterfaces)),
            },
            AttachedInterfaces::new(&interfaces),
            InstantMillis(1_000),
            &mut |bytes| {
                calls += 1;
                let repeated = fixed_secret_key();
                bytes.copy_from_slice(&repeated[..]);
            },
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled {
                    settlement: observed,
                    ..
                }) = reaction
                {
                    settlement = Some(observed);
                }
            },
        );
        assert_eq!(calls, PAIRING_IDENTITY_MINT_ATTEMPTS);
        assert_eq!(
            settlement,
            Some(Settlement::OpenRemoteControlPairing(Err(
                OpenRemoteControlPairingFailure::IdentityGenerationExhausted,
            ))),
        );
        assert_eq!(engine.held_identity_hashes(), &[existing]);
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
    }

    #[test]
    fn registration_failure_releases_the_provisional_identity() {
        type OneDestination = TestFixedStorage<8, 8, 512, 1, 2, 16, 2, 2, 2, 2, 4, 2>;
        let interface = InterfaceId::new([0x95; 8]);
        let interfaces = [routable_descriptor(interface)];
        let mut engine = crate::engine::EngineState::<OneDestination>::default();
        assert_eq!(
            engine.configure_remote_control_pairing(),
            Ok(RemoteControlPairingAvailabilityDestination::canonical()),
        );
        let mut settlement = None;
        let _ = engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(71),
                command: PrnsCommand::OpenRemoteControlPairing(open(EgressTarget::AllInterfaces)),
            },
            AttachedInterfaces::new(&interfaces),
            InstantMillis(1_000),
            &mut fill_pairing_entropy,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled {
                    settlement: observed,
                    ..
                }) = reaction
                {
                    settlement = Some(observed);
                }
            },
        );
        assert_eq!(
            settlement,
            Some(Settlement::OpenRemoteControlPairing(Err(
                OpenRemoteControlPairingFailure::RegisterEndpoint(
                    crate::routing::upstream_app_destinations::RegisterDestinationError::RegistryFull,
                ),
            ))),
        );
        assert_eq!(engine.held_identity_hashes(), &[]);
        assert_eq!(engine.upstream_app_destinations().count(), 1);
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
    }

    #[test]
    fn explicit_close_and_expiry_both_remove_the_endpoint_and_secret() {
        for close_at in [InstantMillis(2_000), InstantMillis(61_000)] {
            let interface = InterfaceId::new([0x96; 8]);
            let interfaces = [routable_descriptor(interface)];
            let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
            assert_eq!(
                engine.configure_remote_control_pairing(),
                Ok(RemoteControlPairingAvailabilityDestination::canonical()),
            );
            let opened = engine
                .open_remote_control_pairing_into(
                    open(EgressTarget::AllInterfaces),
                    AttachedInterfaces::new(&interfaces),
                    InstantMillis(1_000),
                    &mut fill_pairing_entropy,
                    &mut |_| {},
                )
                .unwrap();
            if close_at == InstantMillis(2_000) {
                let mut settlement = None;
                let _ = engine.ingest_command_into(
                    IssuedCommand {
                        id: CommandId(81),
                        command: PrnsCommand::CloseRemoteControlPairing(CloseRemoteControlPairing),
                    },
                    AttachedInterfaces::new(&interfaces),
                    close_at,
                    &mut fill_pairing_entropy,
                    &mut |reaction| {
                        if let EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: observed,
                        }) = reaction
                        {
                            settlement = Some((id, observed));
                        }
                    },
                );
                assert_eq!(
                    settlement,
                    Some((
                        CommandId(81),
                        Settlement::CloseRemoteControlPairing(Ok(
                            CloseRemoteControlPairingOutcome::Closed {
                                endpoint: opened.endpoint,
                            },
                        )),
                    )),
                );
            } else {
                let mut expired = None;
                let schedules = engine.fire_due_remote_control_pairing(
                    close_at,
                    AttachedInterfaces::new(&interfaces),
                    &mut fill_pairing_entropy,
                    &mut |reaction| {
                        if let EngineReaction::Journaled(Journaled::RemoteControlPairingExpired {
                            endpoint,
                        }) = reaction
                        {
                            expired = Some(endpoint);
                        }
                    },
                );
                assert_eq!(
                    schedules.remote_control_pairing,
                    crate::engine::WakeSchedule::Idle,
                );
                assert_eq!(expired, Some(opened.endpoint));
            }
            assert_eq!(
                engine.remote_control_pairing_view(),
                RemoteControlPairingView::Closed,
            );
            assert_eq!(engine.held_identity_hashes(), &[]);
            assert!(!engine.upstream_app_destinations().any(|registration| {
                registration.destination == opened.endpoint.destination_hash()
            }));
        }
    }

    #[test]
    fn automatic_expiry_surfaces_cleanup_failure_and_still_closes_when_endpoint_is_absent() {
        let interface = InterfaceId::new([0x97; 8]);
        let interfaces = [routable_descriptor(interface)];
        let mut engine = crate::engine::EngineState::<TestStorageLayout>::default();
        assert_eq!(
            engine.configure_remote_control_pairing(),
            Ok(RemoteControlPairingAvailabilityDestination::canonical()),
        );
        let opened = engine
            .open_remote_control_pairing_into(
                open(EgressTarget::AllInterfaces),
                AttachedInterfaces::new(&interfaces),
                InstantMillis(1_000),
                &mut fill_pairing_entropy,
                &mut |_| {},
            )
            .unwrap();
        assert!(matches!(
            engine.unregister_destination(&opened.endpoint.destination_hash()),
            crate::engine::UnregisterDestinationOutcome::Unregistered { .. },
        ));
        let mut failure = None;
        let schedules = engine.fire_due_remote_control_pairing(
            InstantMillis(61_000),
            AttachedInterfaces::new(&interfaces),
            &mut fill_pairing_entropy,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RemoteControlPairingExpiryFailed {
                    endpoint,
                    failure: observed,
                }) = reaction
                {
                    failure = Some((endpoint, observed));
                }
            },
        );
        assert_eq!(
            failure,
            Some((
                opened.endpoint,
                CloseRemoteControlPairingFailure::EndpointNotRegistered,
            )),
        );
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed,
        );
        assert_eq!(engine.held_identity_hashes(), &[]);
        assert_eq!(
            schedules.remote_control_pairing,
            crate::engine::WakeSchedule::Idle,
        );
    }
}
