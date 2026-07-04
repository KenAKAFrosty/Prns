use super::*;

pub const MAX_SINGLE_TOKEN_LEN: usize =
    ENCRYPTION_IV_LEN + MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN + 16 + 32;

pub struct DecryptOwed {
    pub destination: DestinationHash,
    pub context: WireContext,
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub identity: IdentityHash,
    pub proof_strategy: ProofStrategy,
    pub packet_hash: PacketHash,
    pub encryption_secret: X25519SecretKey,
    pub recipient_identity_hash: IdentityHash,
    pub ephemeral_public: X25519PublicKey,
    pub token: HeaplessVec<u8, MAX_SINGLE_TOKEN_LEN>,
}

/// How many retained ratchet secrets a deferred decrypt carries to the pool: bounds the
/// per-packet clone only (a packet almost always opens under the newest ratchet, one DH
/// either way). A destination retaining more than this stays on the inline decrypt path.
pub const MAX_POOLED_RATCHETS: usize = 32;

/// The full ciphertext payload a ratcheted decrypt carries: the ephemeral public
/// key plus the token the no-ratchet path splits off.
pub const MAX_RATCHET_DECRYPT_PAYLOAD_LEN: usize =
    ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN + MAX_SINGLE_TOKEN_LEN;

/// A deferred ratcheted decrypt's obligation: the full owned ciphertext payload plus every
/// candidate secret (retained ratchets newest-first, then the identity key), so the pool
/// decrypts-or-drops with no inline fallback. Boxed to keep the crypto-job enum small.
pub struct RatchetDecryptOwed {
    pub destination: DestinationHash,
    pub context: WireContext,
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub identity: IdentityHash,
    pub proof_strategy: ProofStrategy,
    pub packet_hash: PacketHash,
    pub encryption_secret: X25519SecretKey,
    pub ratchet_secrets: HeaplessVec<X25519SecretKey, MAX_POOLED_RATCHETS>,
    pub token: HeaplessVec<u8, MAX_RATCHET_DECRYPT_PAYLOAD_LEN>,
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn maybe_upstream_delivery<'p>(
        &mut self,
        data: DataPacket<'p>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        mut deferred: Option<&mut DeferredCrypto>,
    ) -> Option<(Delivery<'p>, ProofObligation)> {
        if let Some(transport_id) = data.maybe_transport_id {
            if self.transport_id != Some(transport_id) {
                return None;
            }
        }

        match data.destination_type {
            DestinationType::Plain => {
                if received_hops > PLAIN_DATA_MAX_RECEIVED_HOPS {
                    return None;
                }
                self.upstream_app_destinations
                    .lookup(&data.destination, DestinationType::Plain)?;
                Some((
                    Delivery::Plain(PlainDelivery {
                        destination: data.destination,
                        context: data.context,
                        payload: data.payload,
                        arrived_at,
                        source_interface,
                    }),
                    ProofObligation::None,
                ))
            }
            DestinationType::Single => {
                let registered = self
                    .upstream_app_destinations
                    .lookup(&data.destination, DestinationType::Single)?;
                let UpstreamAppDestinationKind::Single {
                    identity,
                    proof_strategy,
                    ..
                } = registered.kind
                else {
                    return None;
                };
                let held = self.held_identities.get(&identity)?;

                let packet_hash = PacketHash::of_data_fields(
                    DestinationType::Single,
                    &data.destination,
                    data.context,
                    data.payload,
                );
                match self.packet_hash_history.remember(packet_hash) {
                    RememberPacketOutcome::AlreadyKnown => return None,
                    RememberPacketOutcome::StoredFresh
                    | RememberPacketOutcome::StoredAfterRotation => {}
                }

                let ratchet_secrets = self.self_ratchets.secrets_newest_first(&data.destination);

                if let Some(deferred) = deferred.as_deref_mut() {
                    if ratchet_secrets.is_empty()
                        && data.payload.len() > ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN
                    {
                        let (ephemeral, token_bytes) =
                            data.payload.split_at(ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN);
                        let mut ephemeral_public_bytes = [0u8; ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN];
                        ephemeral_public_bytes.copy_from_slice(ephemeral);
                        let mut token = HeaplessVec::new();
                        if token.extend_from_slice(token_bytes).is_ok() {
                            deferred.decrypt = Some(DecryptOwed {
                                destination: data.destination,
                                context: data.context,
                                arrived_at,
                                source_interface,
                                identity,
                                proof_strategy,
                                packet_hash,
                                encryption_secret: held.encryption_secret_clone(),
                                recipient_identity_hash: identity,
                                ephemeral_public: X25519PublicKey(ephemeral_public_bytes),
                                token,
                            });
                            return None;
                        }
                    }
                }

                if let Some(deferred) = deferred {
                    if !ratchet_secrets.is_empty()
                        && ratchet_secrets.len() <= MAX_POOLED_RATCHETS
                        && data.payload.len() > ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN
                    {
                        let mut secrets = HeaplessVec::new();
                        let mut token = HeaplessVec::new();
                        if ratchet_secrets
                            .iter()
                            .try_for_each(|secret| secrets.push(secret.cloned()).map_err(|_| ()))
                            .is_ok()
                            && token.extend_from_slice(data.payload).is_ok()
                        {
                            deferred.ratchet_decrypt = Some(RatchetDecryptOwed {
                                destination: data.destination,
                                context: data.context,
                                arrived_at,
                                source_interface,
                                identity,
                                proof_strategy,
                                packet_hash,
                                encryption_secret: held.encryption_secret_clone(),
                                ratchet_secrets: secrets,
                                token,
                            });
                            return None;
                        }
                    }
                }

                let plaintext = held
                    .decrypt_in_place_with_ratchets(ratchet_secrets, data.payload)
                    .ok()?;

                let proof = match proof_strategy {
                    ProofStrategy::ProveAll => ProofObligation::Owed(ProofOwed {
                        packet_hash,
                        identity,
                    }),
                    ProofStrategy::ProveNone => ProofObligation::None,
                    ProofStrategy::ProveIf => ProofObligation::OwedIfApp(ProofOwed {
                        packet_hash,
                        identity,
                    }),
                };
                Some((
                    Delivery::Single(SingleDelivery {
                        destination: data.destination,
                        context: data.context,
                        plaintext,
                        arrived_at,
                        source_interface,
                    }),
                    proof,
                ))
            }
            DestinationType::Group => {
                self.upstream_app_destinations
                    .lookup(&data.destination, DestinationType::Group)?;

                let packet_hash = PacketHash::of_data_fields(
                    DestinationType::Group,
                    &data.destination,
                    data.context,
                    data.payload,
                );
                match self.packet_hash_history.remember(packet_hash) {
                    RememberPacketOutcome::AlreadyKnown => return None,
                    RememberPacketOutcome::StoredFresh
                    | RememberPacketOutcome::StoredAfterRotation => {}
                }

                let key = self.group_keys.key_for(&data.destination)?;
                let token_key = TokenKey::from_derived(key).ok()?;
                let plaintext = token_open_in_place(&token_key, data.payload).ok()?;
                Some((
                    Delivery::Group(GroupDelivery {
                        destination: data.destination,
                        context: data.context,
                        plaintext,
                        arrived_at,
                        source_interface,
                    }),
                    ProofObligation::None,
                ))
            }
            DestinationType::Link => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{
        AnnounceAppData, AnnounceNow, AnnounceTarget, RatchetEntropy, RatchetPolicy,
    };
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::IdentitySigner;
    use crate::routing::announce::derive_destination_hash;
    use crate::routing::ingress::testkit::iface;

    #[test]
    fn a_single_sealed_for_the_announced_destination_is_delivered() {
        let mut state = personal_node_announcer();
        let destination = personal_node_destination();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"hello-announced");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-announced",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_single_sealed_to_the_announced_ratchet_is_delivered() {
        let mut state = ratcheted_personal_node_announcer();
        let destination = personal_node_destination();
        let mut raw = hx(RAW_SEALED_TO_RATCHET);

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"ratchet-parity",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn deferred_ratchet_decrypt_opens_to_the_same_plaintext_as_inline() {
        let mut state = ratcheted_personal_node_announcer();
        let mut raw = hx(RAW_SEALED_TO_RATCHET);
        let mut deferred = DeferredCrypto::default();
        let outcome = state.ingest_packet_with(
            plain_data_packet(&mut raw),
            TEST_ENTROPY,
            &transporting_interfaces(),
            &mut |_| {},
            Some(&mut deferred),
        );
        assert_eq!(outcome, IngestPacketOutcome::OwesRatchetDecrypt);

        let mut owed = deferred
            .ratchet_decrypt
            .expect("the ratcheted single is captured for the pool");
        assert!(
            !owed.ratchet_secrets.is_empty(),
            "the obligation carries the destination's retained ratchets"
        );
        let plaintext = crate::identity::decrypt_token_in_place_with_ratchets(
            &owed.ratchet_secrets,
            &owed.encryption_secret,
            &owed.identity,
            &mut owed.token,
        )
        .expect("a retained ratchet opens the single");
        assert_eq!(plaintext, b"ratchet-parity");
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
                TEST_ANNOUNCE_ENTROPY,
                RatchetEntropy::new([0x77; RatchetEntropy::LEN]),
                &mut buf,
            )
            .written_len();

        let destination = personal_node_destination();
        let mut raw = hx(RAW_SEALED_TO_RATCHET);
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"ratchet-parity",
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
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"identity-keyed",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    const RAW_PLAIN_DATA: &str = "080012f815e3e65add6ceb2fda0e7be338680068656c6c6f2d706c61696e";

    #[test]
    fn neighbor_plain_data_for_a_registered_destination_delivers_the_rns_1_3_5_payload() {
        let mut raw = hx(RAW_PLAIN_DATA);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let destination = state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
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
        let mut raw = hx(RAW_PLAIN_DATA);
        raw[1] = 1;
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn plain_data_for_an_unregistered_destination_is_not_delivered() {
        let mut raw = hx(RAW_PLAIN_DATA);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state
            .register_plain_destination("personal", &["other"])
            .unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn plain_addressed_data_never_reaches_a_single_destination_with_that_hash() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = state.held_identity_hashes()[0];
        let single = state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
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
            destination: single,
            context: WireContext::None,
        };
        let mut raw = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut raw).unwrap();
        raw[header_len] = 0xFF;

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw[..header_len + 1]),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn in_transport_data_delivers_only_when_we_are_the_named_transport_instance() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        let mut raw_for_us = hx(&format!(
            "4800{}{}00{}",
            "4cd0cc45a7405dbd5cf9b5be1ef92f10", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));
        let mut raw_for_other = hx(&format!(
            "4800{}{}00{}",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));

        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Plain(delivered),
            ..
        } = state.ingest_packet(
            plain_data_packet(&mut raw_for_us),
            TEST_ENTROPY,
            &transporting_interfaces(),
        )
        else {
            panic!("in-transport data named to us must deliver plainly");
        };
        assert_eq!(delivered.payload, &[0xEE]);

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw_for_other),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn an_identity_less_relay_never_accepts_in_transport_data() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        let mut raw = hx(&format!(
            "4800{}{}00{}",
            "4cd0cc45a7405dbd5cf9b5be1ef92f10", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn single_data_decrypts_in_place_and_delivers_the_plaintext() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let mut raw = sealed_single_packet(&identity, destination, b"hello-single");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-single",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_replayed_single_packet_is_ignored_by_the_dedup_history() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let raw = sealed_single_packet(&identity, destination, b"hello-single");

        let mut first_copy = raw.clone();
        assert!(matches!(
            state.ingest_packet(
                plain_data_packet(&mut first_copy),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(_),
                ..
            },
        ));

        let mut replayed_copy = raw.clone();
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut replayed_copy),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_tampered_single_token_is_ignored_without_poisoning_the_real_packet() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let raw = sealed_single_packet(&identity, destination, b"hello-single");

        let mut tampered = raw.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut tampered),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );

        let mut genuine = raw.clone();
        assert!(matches!(
            state.ingest_packet(
                plain_data_packet(&mut genuine),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(_),
                ..
            },
        ));
    }

    #[test]
    fn each_single_destination_decrypts_only_under_its_own_held_identity() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
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
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let mut to_a = sealed_single_packet(&identity_a, dest_a, b"for-a");
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut to_a),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: dest_a,
                    context: WireContext::None,
                    plaintext: b"for-a",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );

        let mut to_b = sealed_single_packet(&identity_b, dest_b, b"for-b");
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut to_b),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: dest_b,
                    context: WireContext::None,
                    plaintext: b"for-b",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );

        let mut crossed = sealed_single_packet(&identity_b, dest_a, b"crossed");
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut crossed),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_held_app_identity_does_not_answer_transport_addressed_data() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
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
            state.ingest_packet(
                plain_data_packet(&mut as_app_only),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );

        state.set_transport_identity(&held).unwrap();
        let mut as_transport = raw.clone();
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut as_transport),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-single",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_group_delivery_decrypts_with_the_shared_key_byte_for_byte_vs_rns_1_3_5() {
        // Vector minted live against Python RNS 1.3.5: a GROUP destination held
        // by identity 4cd0cc45… under the app name personal.group, carrying the
        // fixed AES-256 key below, encrypting b"group-hello".
        const GROUP_KEY: &str = "42424242424242424242424242424242424242424242424242424242424242422424242424242424242424242424242424242424242424242424242424242424";
        const GROUP_TOKEN: &str = "614e1126ead06d77c97bdb042c1445d74288ac0645f40cdcdc67a949a0bce8212a4f3524305a78ae9cf89e9a8c302aa2b276c3914b9c3b60d8c41226a22aefcf";

        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_group_destination(
                &identity.identity_hash(),
                "personal",
                &["group"],
                &hx(GROUP_KEY),
            )
            .unwrap();
        assert_eq!(
            destination,
            DestinationHash::new(hx("4b31bea5e2b9b8f6ab79f8ae27a58319").try_into().unwrap()),
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
            destination,
            context: WireContext::None,
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut wire).unwrap();
        let token = hx(GROUP_TOKEN);
        wire[header_len..header_len + token.len()].copy_from_slice(&token);
        let mut raw = wire[..header_len + token.len()].to_vec();

        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Group(group),
            proof: ProofObligation::None,
        } = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: iface(0x07),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_interfaces(),
        )
        else {
            panic!("a GROUP packet for our registered group delivers, owing no proof");
        };
        assert_eq!(group.plaintext, b"group-hello");
        assert_eq!(group.destination, destination);
    }

    #[test]
    fn a_group_packet_for_an_unregistered_group_is_ignored() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Group,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination: DestinationHash::new([0x99; 16]),
            context: WireContext::None,
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut wire).unwrap();
        wire[header_len..header_len + 64].fill(0xAB);
        let mut raw = wire[..header_len + 64].to_vec();
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0x07),
                    bytes: &mut raw,
                },
                TEST_ENTROPY,
                &transporting_interfaces(),
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_prove_all_delivery_carries_the_owed_proof() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let mut raw = sealed_single_packet(&identity, destination, b"prove-me");
        let packet_hash = PacketHash::of_wire_packet(&raw).unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"prove-me",
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
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let registered = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["other"],
                b"",
                ProofStrategy::ProveNone,
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
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_interfaces()
            ),
            IngestPacketOutcome::Ignored,
        );
    }
}
