use super::*;
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::manifold::tokio::TokioHost;
use prns_core::entropy::RuntimeEntropy;

pub fn secrets(index: usize) -> RemoteControlNodeIdentitySecrets {
    RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
            [0x41 + index as u8 * 2; 64],
        )),
        RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
            [0x42 + index as u8 * 2; 64],
        )),
    )
    .expect("distinct validation keys")
}
pub fn target(index: usize) -> RemoteControlTargetIdentity {
    RemoteControlTargetIdentity::new(*secrets(index).identities().target().public_keys())
}
pub fn controller(index: usize) -> IdentityHash {
    secrets(index).identities().controller().identity_hash()
}
pub fn page(index: usize) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "simulation",
        aspects: &["halow", "page"],
        identity: personal_rns::identity::Zeroizing::new([0x61 + index as u8; 64]),
        announce_app_data: b"",
        proof: personal_rns::prelude::ProofStrategy::ProveAll,
        link_requests: personal_rns::prelude::LinkRequestPolicy::AcceptAll,
        ratchet: personal_rns::engine::RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::Yes,
    }
}
pub fn supervisor(
    radio: VirtualHaLowRadio,
    index: usize,
) -> personal_rns::wifi_halow::HaLow<VirtualHaLowRadio> {
    personal_rns::wifi_halow::HaLow::new(
        radio,
        scope(index),
        policy_for_bitrate(BitrateBps::guess(7_300_000)),
        policy_for_bitrate(BitrateBps::guess(4_000_000)),
        personal_rns::wifi_halow::HaLowLimits {
            peers: NonZeroU8::new(16).expect("peer budget"),
            idle_seconds: NonZeroU32::new(IDLE_SECONDS).expect("idle budget"),
        },
    )
}
impl Lab<'_> {
    pub(super) fn start_node(&mut self, index: usize) {
        let radio = self.medium.attach(mac(index)).expect("unique radio");
        let radio_id = radio.id();
        let generation = self.generations[index];
        self.generations[index] = generation + 1;
        let origin = personal_rns::engine::InstantMillis(1_000_000 + self.medium.now().get());
        let calls = self.calls.clone();
        let announces = self.announces.clone();
        let management = match &self.bindings {
            Bindings::ManagedTarget(control) if index == TARGET => {
                Some((self.medium.clone(), control.clone()))
            }
            Bindings::Explicit | Bindings::ManagedTarget(_) => None,
        };
        let persistence = match &self.persistence {
            Persistence::Disabled => persistence::Selection::Disabled,
            Persistence::Retained(stores) => persistence::Selection::Retained(
                NodePersistence::custom_dir(stores[index].directory())
                    .expect("retained state")
                    .with_io_driver(persistence::InlineIo),
            ),
        };
        let grants = match &self.persistence {
            Persistence::Retained(_) if generation != 0 => Vec::new(),
            Persistence::Disabled | Persistence::Retained(_) => [PRIMARY, HEALTHY]
                .iter()
                .map(|controller| {
                    RemoteControlControllerGrant::new(
                        *secrets(*controller).identities().controller(),
                        RemoteControlControllerAuthority::Operator,
                        permissions(),
                    )
                    .expect("explicit grant")
                })
                .collect::<Vec<_>>(),
        };
        let (ready, mut ready_rx) = oneshot::channel();
        let (shutdown, stopping) = oneshot::channel();
        let task = self.insert(async move {
            let initial_grants = match grants.as_slice() {
                [] => RemoteControlInitialControllerGrants::Nobody,
                grants => RemoteControlInitialControllerGrants::Grants(
                    RemoteControlControllerGrants::try_from(grants).expect("bounded grants"),
                ),
            };
            let entropy = |domain: u8| {
                RuntimeEntropy::try_new(move |bytes: &mut [u8]| {
                    for (offset, byte) in bytes.iter_mut().enumerate() {
                        *byte = domain ^ (index as u8 * 17) ^ generation.to_be_bytes()[offset % 8];
                    }
                    Ok::<(), core::convert::Infallible>(())
                })
                .expect("fixed validation entropy")
            };
            let mut path_sequence = 0u64;
            let mut node = PrnsNode::new_with_entropy_sources(
                |handle| PrnsNodeRecipe {
                    remote_control: RemoteControlNodeSetup::new(
                        RemoteControlService::with_capabilities(
                            secrets(index),
                            initial_grants,
                            RemoteControlSelfAnnouncement::Unavailable,
                            RemoteControlCapabilities::from_requests(permissions())
                                .expect("describe capability"),
                        ),
                    )
                    .with_handlers(host::InspectionHost(handle), host::Messages { calls }),
                    transport_identity: Some(personal_rns::identity::Zeroizing::new(
                        [0x91 + index as u8; 64],
                    )),
                    pre_configured_destinations: [page(index)],
                    app_state: (),
                    storage: personal_rns::storage::GrowableHeap,
                    request_endpoints: personal_rns::request_endpoints![
                        crate::traffic::ResourceReply
                    ],
                    on_event: move |event, _: &()| {
                        if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                            destination,
                            ..
                        }) = event
                        {
                            let mut observed = announces.borrow_mut();
                            assert!(observed.len() < 4096, "bounded announce observations");
                            observed.push(AnnounceSeen {
                                node: index,
                                destination,
                            });
                        }
                    },
                    interfaces: ManuallyAttached,
                    persistence,
                },
                TokioHost::with_runtime_entropy(origin, entropy(0xa1)),
                TokioHandleEntropy::from_sources(entropy(0xb1), move |bytes: &mut [u8]| {
                    path_sequence = path_sequence.checked_add(1).expect("path ID budget");
                    for (offset, byte) in bytes.iter_mut().enumerate() {
                        let component = if offset < 8 {
                            path_sequence
                        } else {
                            generation
                        };
                        *byte = 0xc1 ^ (index as u8 * 17) ^ component.to_be_bytes()[offset % 8];
                    }
                    Ok::<(), core::convert::Infallible>(())
                }),
            )
            .with_crypto_pool(CryptoPoolConfig::Inline)
            .with_interface_arbitration(InterfaceArbitration::RoundRobin {
                first: InterfaceEventSource::Message,
            });
            let handle = node.handle();
            node.register_request_route::<crate::traffic::ResourceReply>(
                &target(index).endpoint().destination_hash(),
            )
            .expect("resource route");
            let attached = match management {
                Some((medium, control)) => {
                    handle.supervise(personal_rns::wifi_halow::HaLowDevice::new(
                        device::Source::new(radio, medium, control),
                        scope(index),
                        policy_for_bitrate(BitrateBps::guess(7_300_000)),
                        policy_for_bitrate(BitrateBps::guess(4_000_000)),
                        personal_rns::wifi_halow::HaLowLimits {
                            peers: NonZeroU8::new(16).expect("peers"),
                            idle_seconds: NonZeroU32::new(IDLE_SECONDS).expect("idle"),
                        },
                        personal_rns::prelude::ReconnectPolicy::STANDARD,
                    ))
                }
                None => handle.supervise(supervisor(radio, index)),
            };
            ready
                .send((handle, attached))
                .unwrap_or_else(|_| panic!("ready receiver"));
            Event::Stopped {
                node: index,
                result: node
                    .run_until(async {
                        let _ = stopping.await;
                    })
                    .await,
            }
        });
        assert!(self.settle().is_empty());
        let (handle, supervisor) = ready_rx.try_recv().expect("initialized node");
        self.nodes.push(Node {
            handle,
            radio: radio_id,
            adapter: AdapterState::Attached(supervisor),
            task,
            shutdown,
        });
    }
    pub fn replace_adapter(&mut self, index: usize) {
        self.detach_adapter(index);
        let radio = self.medium.attach(mac(index)).expect("replacement radio");
        self.nodes[index].radio = radio.id();
        self.nodes[index].adapter =
            AdapterState::Attached(self.nodes[index].handle.supervise(supervisor(radio, index)));
        assert!(self.settle().is_empty());
    }
    pub fn detach_adapter(&mut self, index: usize) {
        if let AdapterState::Attached(attached) =
            std::mem::replace(&mut self.nodes[index].adapter, AdapterState::Detached)
        {
            attached.teardown();
        }
        assert!(self.settle().is_empty());
    }
    pub fn restart(&mut self, index: usize) {
        self.runner
            .cancel(self.nodes[index].task)
            .expect("retire old node actor");
        self.start_node(index);
        let last = self.nodes.len() - 1;
        self.nodes.swap(index, last);
        self.nodes.pop().expect("discard old handles");
    }
}
