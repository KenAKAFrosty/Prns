use super::classification::DataPacket;
use super::dispatch::IngressCryptoMode;
use crate::crypto::ratchets::RatchetPolicy;
use crate::crypto::{sealed_len, token_open_in_place, X25519PublicKey, X25519SecretKey};
use crate::engine::{EngineState, InstantMillis, MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN};
use crate::identity::{IdentityHash, IdentityKeyFallback, ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN};
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::{Delivery, GroupDelivery, PlainDelivery, SingleDelivery};
use crate::routing::proof::{ProofObligation, ProofOwed};
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, DestinationType, WireContext};
use heapless::Vec as HeaplessVec;

pub const MAX_SINGLE_TOKEN_LEN: usize = sealed_len(MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN);

/// Owns the material needed to resume an identity-keyed decrypt after external Diffie-Hellman.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub struct DecryptOwed {
    pub destination: DestinationHash,
    pub context: WireContext,
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub identity: IdentityHash,
    pub proof_strategy: ProofStrategy,
    pub packet_hash: PacketHash,
    pub encryption_secret: X25519SecretKey,
    pub ephemeral_public: X25519PublicKey,
    pub token: HeaplessVec<u8, MAX_SINGLE_TOKEN_LEN>,
}

/// Limits retained secrets copied into a ratchet-decrypt request; larger sets decrypt inline.
pub const MAX_POOLED_RATCHETS: usize = 32;

pub const MAX_RATCHET_DECRYPT_PAYLOAD_LEN: usize =
    ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN + MAX_SINGLE_TOKEN_LEN;

/// Owns the ciphertext and candidate secrets needed to resume a ratchet decrypt.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub struct RatchetDecryptOwed {
    pub destination: DestinationHash,
    pub context: WireContext,
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub identity: IdentityHash,
    pub proof_strategy: ProofStrategy,
    pub packet_hash: PacketHash,
    pub encryption_secret: X25519SecretKey,
    pub identity_key_fallback: IdentityKeyFallback,
    pub ratchet_secrets: HeaplessVec<X25519SecretKey, MAX_POOLED_RATCHETS>,
    pub token: HeaplessVec<u8, MAX_RATCHET_DECRYPT_PAYLOAD_LEN>,
}

// The owned ratchet continuation is deliberately movable out of the no-alloc engine. Boxing it
// here would make a pure protocol classification allocate.
#[allow(clippy::large_enum_variant)]
pub(super) enum UpstreamDeliveryOutcome<'p> {
    Delivered(Delivery<'p>, ProofObligation),
    OwesDecrypt(DecryptOwed),
    OwesRatchetDecrypt(RatchetDecryptOwed),
    NotForUs,
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn maybe_upstream_delivery<'p>(
        &mut self,
        data: DataPacket<'p>,
        packet_hash: PacketHash,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        crypto: IngressCryptoMode,
    ) -> UpstreamDeliveryOutcome<'p> {
        let destination = DestinationHash::from_address(data.header.address);
        match data.header.destination_type {
            DestinationType::Plain => {
                if self
                    .upstream_app_destinations
                    .lookup(&destination, DestinationType::Plain)
                    .is_none()
                {
                    return UpstreamDeliveryOutcome::NotForUs;
                }
                UpstreamDeliveryOutcome::Delivered(
                    Delivery::Plain(PlainDelivery {
                        destination,
                        context: data.header.context,
                        payload: data.payload,
                        arrived_at,
                        source_interface,
                    }),
                    ProofObligation::None,
                )
            }
            DestinationType::Single => {
                let Some(registered) = self.upstream_app_destinations.lookup_single(&destination)
                else {
                    return UpstreamDeliveryOutcome::NotForUs;
                };
                let Some(held) = self.held_identities.get(&registered.identity) else {
                    return UpstreamDeliveryOutcome::NotForUs;
                };
                let identity_key_fallback = match registered.ratchet_policy {
                    RatchetPolicy::NoRatchets | RatchetPolicy::Ratcheted => {
                        IdentityKeyFallback::Permitted
                    }
                    RatchetPolicy::RatchetsRequired => IdentityKeyFallback::Refused,
                };

                let ratchet_secrets = self.self_ratchets.secrets_newest_first(&destination);

                if crypto == IngressCryptoMode::Owed
                    && data.payload.len() > ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN
                {
                    if ratchet_secrets.is_empty()
                        && identity_key_fallback == IdentityKeyFallback::Permitted
                    {
                        let (ephemeral, token_bytes) =
                            data.payload.split_at(ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN);
                        let mut ephemeral_public_bytes = [0u8; ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN];
                        ephemeral_public_bytes.copy_from_slice(ephemeral);
                        let mut token = HeaplessVec::new();
                        if token.extend_from_slice(token_bytes).is_ok() {
                            return UpstreamDeliveryOutcome::OwesDecrypt(DecryptOwed {
                                destination,
                                context: data.header.context,
                                arrived_at,
                                source_interface,
                                identity: registered.identity,
                                proof_strategy: registered.proof_strategy,
                                packet_hash,
                                encryption_secret: held.encryption_secret_clone(),
                                ephemeral_public: X25519PublicKey(ephemeral_public_bytes),
                                token,
                            });
                        }
                    } else if !ratchet_secrets.is_empty()
                        && ratchet_secrets.len() <= MAX_POOLED_RATCHETS
                    {
                        let mut secrets = HeaplessVec::new();
                        let mut token = HeaplessVec::new();
                        if ratchet_secrets
                            .iter()
                            .try_for_each(|secret| secrets.push(secret.cloned()).map_err(|_| ()))
                            .is_ok()
                            && token.extend_from_slice(data.payload).is_ok()
                        {
                            return UpstreamDeliveryOutcome::OwesRatchetDecrypt(
                                RatchetDecryptOwed {
                                    destination,
                                    context: data.header.context,
                                    arrived_at,
                                    source_interface,
                                    identity: registered.identity,
                                    proof_strategy: registered.proof_strategy,
                                    packet_hash,
                                    encryption_secret: held.encryption_secret_clone(),
                                    identity_key_fallback,
                                    ratchet_secrets: secrets,
                                    token,
                                },
                            );
                        }
                    }
                }

                let Ok(opened) = held.decrypt_in_place_with_ratchets(
                    ratchet_secrets,
                    identity_key_fallback,
                    data.payload,
                ) else {
                    return UpstreamDeliveryOutcome::NotForUs;
                };

                UpstreamDeliveryOutcome::Delivered(
                    Delivery::Single(SingleDelivery {
                        destination,
                        context: data.header.context,
                        plaintext: opened.plaintext,
                        opened_by: opened.opened_by,
                        arrived_at,
                        source_interface,
                    }),
                    ProofObligation::for_delivery(
                        registered.proof_strategy,
                        ProofOwed {
                            packet_hash,
                            identity: registered.identity,
                        },
                    ),
                )
            }
            DestinationType::Group => {
                if self
                    .upstream_app_destinations
                    .lookup(&destination, DestinationType::Group)
                    .is_none()
                {
                    return UpstreamDeliveryOutcome::NotForUs;
                }

                let Some(key) = self.group_keys.key_for(&destination) else {
                    return UpstreamDeliveryOutcome::NotForUs;
                };
                let Ok(plaintext) = token_open_in_place(&key.as_token_key(), data.payload) else {
                    return UpstreamDeliveryOutcome::NotForUs;
                };
                UpstreamDeliveryOutcome::Delivered(
                    Delivery::Group(GroupDelivery {
                        destination,
                        context: data.header.context,
                        plaintext,
                        arrived_at,
                        source_interface,
                    }),
                    ProofObligation::None,
                )
            }
            DestinationType::Link => UpstreamDeliveryOutcome::NotForUs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ratchets::RatchetId;
    use crate::crypto::x25519_diffie_hellman;
    use crate::engine::test_support::*;
    use crate::engine::{
        AnnounceAppData, AnnounceNow, AnnounceTarget, EngineReaction, Journaled, RatchetPolicy,
    };
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::{IdentitySigner, OpenedBy};
    use crate::interfaces::{AttachedInterfaces, InboundPacket};
    use crate::routing::announce::derive_destination_hash;
    use crate::routing::ingress::testkit::iface;
    use crate::routing::ingress::{IgnoreReason, IngestPacketOutcome};
    use crate::routing::upstream_app_destinations::LinkRequestPolicy;
    use crate::wire::{
        ContextFlag, IfacFlag, PacketType, PropagationType, TransportId, WireAddress,
        WirePacketHeader, BROADCAST_MTU,
    };

    fn announced_ratchet_id() -> RatchetId {
        RatchetId::of_secret(&X25519SecretKey::new([0x55; 32]))
    }

    #[test]
    fn a_single_sealed_for_the_announced_destination_is_delivered() {
        let mut state = personal_node_announcer();
        let destination = personal_node_destination();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"hello-announced");

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-announced",
                    opened_by: OpenedBy::IdentityKey,
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn owed_identity_decrypt_resumes_the_delivery() {
        let mut state = personal_node_announcer();
        let destination = personal_node_destination();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"hello-deferred");
        let IngestPacketOutcome::OwesDecrypt(owed) = state.ingest_packet_step_with(
            plain_data_packet(&mut raw),
            AttachedInterfaces::new(&transporting_interfaces()),
        ) else {
            panic!("the identity-keyed single owes a decrypt operation");
        };
        let shared = x25519_diffie_hellman(&owed.encryption_secret, &owed.ephemeral_public);
        let mut delivery = None;
        let interfaces = transporting_interfaces();
        state.resume_decrypt(
            owed,
            shared,
            AttachedInterfaces::new(&interfaces),
            &mut |_| false,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::Delivered(Delivery::Single(single))) =
                    reaction
                {
                    delivery = Some((
                        single.destination,
                        single.context,
                        single.plaintext.to_vec(),
                        single.opened_by,
                        single.arrived_at,
                        single.source_interface,
                    ));
                }
            },
        );

        assert_eq!(
            delivery,
            Some((
                destination,
                WireContext::None,
                b"hello-deferred".to_vec(),
                OpenedBy::IdentityKey,
                InstantMillis(1_000),
                InterfaceId::new([0x07; 8]),
            )),
        );
    }

    #[test]
    fn a_single_sealed_to_the_announced_ratchet_is_delivered() {
        let mut state = ratcheted_personal_node_announcer();
        let destination = personal_node_destination();
        let mut raw = bytes_from_hex(RNS_1_4_2_SEALED_TO_RATCHET);

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"ratchet-parity",
                    opened_by: OpenedBy::Ratchet(announced_ratchet_id()),
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn owed_ratchet_decrypt_opens_to_the_same_plaintext_as_inline() {
        let mut state = ratcheted_personal_node_announcer();
        let mut raw = bytes_from_hex(RNS_1_4_2_SEALED_TO_RATCHET);
        let IngestPacketOutcome::OwesRatchetDecrypt(mut owed) = state.ingest_packet_step_with(
            plain_data_packet(&mut raw),
            AttachedInterfaces::new(&transporting_interfaces()),
        ) else {
            panic!("the ratcheted single owes a ratchet decrypt operation");
        };
        assert!(
            !owed.ratchet_secrets.is_empty(),
            "the obligation carries the destination's retained ratchets"
        );
        let (opened_by, plaintext) = {
            let opened = crate::identity::decrypt_token_in_place_with_ratchets(
                &owed.ratchet_secrets,
                &owed.encryption_secret,
                &owed.identity,
                owed.identity_key_fallback,
                &mut owed.token,
            )
            .expect("a retained ratchet opens the single");
            assert_eq!(opened.plaintext, b"ratchet-parity");
            assert_eq!(opened.opened_by, OpenedBy::Ratchet(announced_ratchet_id()));
            (opened.opened_by, opened.plaintext.to_vec())
        };
        let mut delivery = None;
        let interfaces = transporting_interfaces();
        state.resume_ratchet_decrypt(
            owed,
            crate::identity::OpenedToken {
                opened_by,
                plaintext: &plaintext,
            },
            AttachedInterfaces::new(&interfaces),
            &mut |_| false,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::Delivered(Delivery::Single(single))) =
                    reaction
                {
                    delivery = Some((
                        single.destination,
                        single.plaintext.to_vec(),
                        single.opened_by,
                    ));
                }
            },
        );
        assert_eq!(
            delivery,
            Some((
                personal_node_destination(),
                b"ratchet-parity".to_vec(),
                OpenedBy::Ratchet(announced_ratchet_id()),
            )),
        );
    }

    #[test]
    fn an_earlier_announced_ratchet_still_opens_after_rotation() {
        let mut state = ratcheted_personal_node_announcer();
        let interval = 6 * 60 * 60 * 1000;
        let mut buf = [0u8; BROADCAST_MTU];
        let _ = state
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(1_000 + interval),
                &mut |bytes: &mut [u8]| bytes.fill(0x77),
                &mut buf,
            )
            .written_len();

        let destination = personal_node_destination();
        let mut raw = bytes_from_hex(RNS_1_4_2_SEALED_TO_RATCHET);
        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"ratchet-parity",
                    opened_by: OpenedBy::Ratchet(announced_ratchet_id()),
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_ratcheted_destination_still_opens_identity_keyed_traffic() {
        let mut state = ratcheted_personal_node_announcer();
        let destination = personal_node_destination();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"identity-keyed");

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"identity-keyed",
                    opened_by: OpenedBy::IdentityKey,
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    fn ratchets_required_personal_node_announcer() -> EngineState<TestStorageLayout> {
        let mut state = personal_node_announcer_with(RatchetPolicy::RatchetsRequired);
        let mut buf = [0u8; BROADCAST_MTU];
        let _ = state.write_commanded_announce(
            &AnnounceNow {
                destination: personal_node_destination(),
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            },
            InstantMillis(1_000),
            &mut |bytes: &mut [u8]| bytes.fill(0x55),
            &mut buf,
        );
        state
    }

    #[test]
    fn a_ratchets_required_destination_refuses_identity_keyed_traffic() {
        let mut state = ratchets_required_personal_node_announcer();
        let destination = personal_node_destination();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"identity-keyed");

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    #[test]
    fn a_ratchets_required_destination_still_delivers_ratcheted_traffic() {
        let mut state = ratchets_required_personal_node_announcer();
        let destination = personal_node_destination();
        let mut raw = bytes_from_hex(RNS_1_4_2_SEALED_TO_RATCHET);

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"ratchet-parity",
                    opened_by: OpenedBy::Ratchet(announced_ratchet_id()),
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn an_owed_required_ratchet_decrypt_carries_the_refusal_to_the_runtime() {
        let mut state = ratchets_required_personal_node_announcer();
        let destination = personal_node_destination();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"identity-keyed");
        let IngestPacketOutcome::OwesRatchetDecrypt(mut owed) = state.ingest_packet_step_with(
            plain_data_packet(&mut raw),
            AttachedInterfaces::new(&transporting_interfaces()),
        ) else {
            panic!("the required-ratchet single owes a ratchet decrypt operation");
        };
        assert_eq!(owed.identity_key_fallback, IdentityKeyFallback::Refused);
        assert_eq!(
            crate::identity::decrypt_token_in_place_with_ratchets(
                &owed.ratchet_secrets,
                &owed.encryption_secret,
                &owed.identity,
                owed.identity_key_fallback,
                &mut owed.token,
            ),
            Err(crate::identity::DecryptError::RatchetRequired),
        );
    }

    #[test]
    fn a_ratchets_required_destination_never_defers_to_the_identity_key_pool() {
        let mut state = personal_node_announcer_with(RatchetPolicy::RatchetsRequired);
        let destination = personal_node_destination();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"identity-keyed");
        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    const RAW_PLAIN_DATA: &str = "080012f815e3e65add6ceb2fda0e7be338680068656c6c6f2d706c61696e";

    #[test]
    fn neighbor_plain_data_for_a_registered_destination_delivers_the_rns_1_4_2_payload() {
        let mut raw = bytes_from_hex(RAW_PLAIN_DATA);
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let destination = state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Plain(PlainDelivery {
                    destination,
                    context: WireContext::None,
                    payload: b"hello-plain",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn relayed_plain_data_is_dropped_at_the_packet_filter() {
        let mut raw = bytes_from_hex(RAW_PLAIN_DATA);
        raw[1] = 1;
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::HopLimitReached),
        );
    }

    #[test]
    fn plain_data_for_an_unregistered_destination_is_not_delivered() {
        let mut raw = bytes_from_hex(RAW_PLAIN_DATA);
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        state
            .register_plain_destination("personal", &["other"])
            .unwrap();

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    #[test]
    fn plain_addressed_data_never_reaches_a_single_destination_with_that_hash() {
        let mut state: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
        let node = state.held_identity_hashes()[0];
        let single = state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            address: single.to_address(),
            context: WireContext::None,
        };
        let mut raw = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut raw).unwrap();
        raw[header_len] = 0xFF;

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw[..header_len + 1]),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    #[test]
    fn in_transport_data_delivers_only_when_we_are_the_named_transport_instance() {
        let mut state: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        let mut raw_for_us = bytes_from_hex(&format!(
            "4800{}{}00{}",
            "4cd0cc45a7405dbd5cf9b5be1ef92f10", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));
        let mut raw_for_other = bytes_from_hex(&format!(
            "4800{}{}00{}",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));

        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Plain(delivered),
            ..
        } = state.ingest_for_test(
            plain_data_packet(&mut raw_for_us),
            AttachedInterfaces::new(&transporting_interfaces()),
        )
        else {
            panic!("in-transport data named to us must deliver plainly");
        };
        assert_eq!(delivered.payload, &[0xEE]);

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw_for_other),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::OtherInstance),
        );
    }

    #[test]
    fn an_identity_less_relay_never_accepts_in_transport_data() {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        let mut raw = bytes_from_hex(&format!(
            "4800{}{}00{}",
            "4cd0cc45a7405dbd5cf9b5be1ef92f10", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::OtherInstance),
        );
    }

    #[test]
    fn single_data_decrypts_in_place_and_delivers_the_plaintext() {
        let mut state: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let mut raw = sealed_single_packet(&identity, destination, b"hello-single");

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-single",
                    opened_by: OpenedBy::IdentityKey,
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_replayed_single_packet_is_ignored_by_the_dedup_history() {
        let mut state: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let raw = sealed_single_packet(&identity, destination, b"hello-single");

        let mut first_copy = raw.clone();
        assert!(matches!(
            state.ingest_for_test(
                plain_data_packet(&mut first_copy),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(_),
                ..
            },
        ));

        let mut replayed_copy = raw.clone();
        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut replayed_copy),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::Duplicate),
        );
    }

    #[test]
    fn a_tampered_single_token_is_ignored_without_poisoning_the_real_packet() {
        let mut state: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let raw = sealed_single_packet(&identity, destination, b"hello-single");

        let mut tampered = raw.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut tampered),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );

        let mut genuine = raw.clone();
        assert!(matches!(
            state.ingest_for_test(
                plain_data_packet(&mut genuine),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(_),
                ..
            },
        ));
    }

    #[test]
    fn each_single_destination_decrypts_only_under_its_own_held_identity() {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let identity_a = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let identity_b = InMemoryNodeIdentity::from_secret_key_bytes(&second_secret_key());
        let held_a = state.hold_identity(fixed_secret_key()).unwrap();
        let held_b = state.hold_identity(second_secret_key()).unwrap();
        assert_eq!(held_a, identity_a.identity_hash());
        assert_eq!(held_b, identity_b.identity_hash());

        let dest_a = state
            .register_single_destination(
                &held_a,
                "personal",
                &["a"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let dest_b = state
            .register_single_destination(
                &held_b,
                "personal",
                &["b"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let mut to_a = sealed_single_packet(&identity_a, dest_a, b"for-a");
        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut to_a),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: dest_a,
                    context: WireContext::None,
                    plaintext: b"for-a",
                    opened_by: OpenedBy::IdentityKey,
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );

        let mut to_b = sealed_single_packet(&identity_b, dest_b, b"for-b");
        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut to_b),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: dest_b,
                    context: WireContext::None,
                    plaintext: b"for-b",
                    opened_by: OpenedBy::IdentityKey,
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );

        let mut crossed = sealed_single_packet(&identity_b, dest_a, b"crossed");
        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut crossed),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    #[test]
    fn a_held_app_identity_does_not_answer_transport_addressed_data() {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let raw = sealed_single_packet_routed(
            &identity,
            Some(TransportId::new(*held.as_bytes())),
            destination,
            b"hello-single",
        );

        let mut as_app_only = raw.clone();
        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut as_app_only),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::OtherInstance),
        );

        state.set_transport_identity(&held).unwrap();
        let mut as_transport = raw.clone();
        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut as_transport),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-single",
                    opened_by: OpenedBy::IdentityKey,
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_group_delivery_decrypts_with_the_shared_key_byte_for_byte_vs_rns_1_4_2() {
        // Python RNS GROUP vector minted with 1.3.5 and revalidated with 1.4.2, for the fixed AES-256 key below, app name personal.group, and plaintext b"group-hello".
        const GROUP_KEY: &str = "42424242424242424242424242424242424242424242424242424242424242422424242424242424242424242424242424242424242424242424242424242424";
        const GROUP_TOKEN: &str = "614e1126ead06d77c97bdb042c1445d74288ac0645f40cdcdc67a949a0bce8212a4f3524305a78ae9cf89e9a8c302aa2b276c3914b9c3b60d8c41226a22aefcf";

        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_group_destination(
                &identity.identity_hash(),
                "personal",
                &["group"],
                &bytes_from_hex(GROUP_KEY),
            )
            .unwrap();
        assert_eq!(
            destination,
            DestinationHash::new(
                bytes_from_hex("4b31bea5e2b9b8f6ab79f8ae27a58319")
                    .try_into()
                    .unwrap()
            ),
            "our GROUP address derivation matches RNS Destination.hash",
        );

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Group,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            address: destination.to_address(),
            context: WireContext::None,
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut wire).unwrap();
        let token = bytes_from_hex(GROUP_TOKEN);
        wire[header_len..header_len + token.len()].copy_from_slice(&token);
        let mut raw = wire[..header_len + token.len()].to_vec();

        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Group(group),
            proof: ProofObligation::None,
        } = state.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: iface(0x07),
                bytes: &mut raw,
            },
            AttachedInterfaces::new(&transporting_interfaces()),
        )
        else {
            panic!("a GROUP packet for our registered group delivers, owing no proof");
        };
        assert_eq!(group.plaintext, b"group-hello");
        assert_eq!(group.destination, destination);
    }

    #[test]
    fn a_group_packet_for_an_unregistered_group_is_ignored() {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Group,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            address: WireAddress::new([0x99; 16]),
            context: WireContext::None,
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut wire).unwrap();
        wire[header_len..header_len + 64].fill(0xAB);
        let mut raw = wire[..header_len + 64].to_vec();
        assert_eq!(
            state.ingest_for_test(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0x07),
                    bytes: &mut raw,
                },
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }

    fn registered_group() -> (EngineState<TestStorageLayout>, DestinationHash) {
        const GROUP_KEY: &str = "42424242424242424242424242424242424242424242424242424242424242422424242424242424242424242424242424242424242424242424242424242424";
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_group_destination(
                &identity.identity_hash(),
                "personal",
                &["group"],
                &bytes_from_hex(GROUP_KEY),
            )
            .unwrap();
        (state, destination)
    }

    fn group_wire(destination: DestinationHash, hops: u8) -> std::vec::Vec<u8> {
        const GROUP_TOKEN: &str = "614e1126ead06d77c97bdb042c1445d74288ac0645f40cdcdc67a949a0bce8212a4f3524305a78ae9cf89e9a8c302aa2b276c3914b9c3b60d8c41226a22aefcf";
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Group,
            packet_type: PacketType::Data,
            hops,
            transport_id: None,
            address: destination.to_address(),
            context: WireContext::None,
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut wire).unwrap();
        let token = bytes_from_hex(GROUP_TOKEN);
        wire[header_len..header_len + token.len()].copy_from_slice(&token);
        wire[..header_len + token.len()].to_vec()
    }

    #[test]
    fn a_group_packet_delivers_from_a_direct_neighbor_but_drops_once_relayed() {
        let (mut state, destination) = registered_group();
        let mut direct = group_wire(destination, 0);
        assert!(
            matches!(
                state.ingest_for_test(
                    InboundPacket {
                        arrived_at: InstantMillis(1_000),
                        source_interface: iface(0x07),
                        bytes: &mut direct,
                    },
                    AttachedInterfaces::new(&transporting_interfaces()),
                ),
                IngestPacketOutcome::Delivery {
                    delivery: Delivery::Group(_),
                    ..
                }
            ),
            "a GROUP packet received from a direct neighbor (one hop) delivers",
        );

        let mut relayed = group_wire(destination, 1);
        assert_eq!(
            state.ingest_for_test(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0x07),
                    bytes: &mut relayed,
                },
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::HopLimitReached),
            "a GROUP packet relayed beyond one hop is dropped, matching RNS packet_filter",
        );
    }

    #[test]
    fn a_group_packet_is_not_deduplicated_matching_rns() {
        let (mut state, destination) = registered_group();
        let mut first = group_wire(destination, 0);
        let mut second = group_wire(destination, 0);
        assert!(
            matches!(
                state.ingest_for_test(
                    InboundPacket {
                        arrived_at: InstantMillis(1_000),
                        source_interface: iface(0x07),
                        bytes: &mut first,
                    },
                    AttachedInterfaces::new(&transporting_interfaces()),
                ),
                IngestPacketOutcome::Delivery {
                    delivery: Delivery::Group(_),
                    ..
                }
            ),
            "the first copy of a GROUP packet delivers",
        );
        assert!(
            matches!(
                state.ingest_for_test(
                    InboundPacket {
                        arrived_at: InstantMillis(2_000),
                        source_interface: iface(0x07),
                        bytes: &mut second,
                    },
                    AttachedInterfaces::new(&transporting_interfaces()),
                ),
                IngestPacketOutcome::Delivery {
                    delivery: Delivery::Group(_),
                    ..
                }
            ),
            "an identical second copy still delivers: the transport hashlist does not cover GROUP",
        );
    }

    #[test]
    fn a_prove_all_delivery_carries_the_owed_proof() {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let mut raw = sealed_single_packet(&identity, destination, b"prove-me");
        let packet_hash = PacketHash::of_wire_packet(&raw).unwrap();

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"prove-me",
                    opened_by: OpenedBy::IdentityKey,
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::Owed(ProofOwed {
                    packet_hash,
                    identity: held,
                }),
            },
        );
    }

    #[test]
    fn single_data_for_an_unregistered_destination_is_ignored() {
        let mut state: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let registered = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["other"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let unregistered = derive_destination_hash(
            &identity.identity_hash(),
            &crate::routing::announce::expand_name("personal", &["node"]).unwrap(),
        );
        assert_ne!(registered, unregistered);
        let mut raw = sealed_single_packet(&identity, unregistered, b"hello-single");

        assert_eq!(
            state.ingest_for_test(
                plain_data_packet(&mut raw),
                AttachedInterfaces::new(&transporting_interfaces()),
            ),
            IngestPacketOutcome::Ignored(IgnoreReason::NotForUs),
        );
    }
}
