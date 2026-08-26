use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::stream::{FuturesUnordered, StreamExt};
use futures_util::FutureExt;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::engine::Departure;
use crate::interfaces::IfacContext;
use crate::interfaces::{
    ConnectionView, FrameAccounting, FrameAccountingRecorder, InterfaceDescriptor, InterfaceId,
    InterfaceKind, InterfaceOriginKind, InterfaceSnapshot, Membership, ReportsStatus, StatusView,
};
use crate::manifold::driver::{
    tokio_grant_lane, AddInterfaceCommand, HostCommand, TokioInterfaceSeam,
};
use crate::manifold::interface_seam::{frame_cap_for, Interface};
use crate::node_introspection::{
    FrameAccountingCoverage, InterfaceIfacSnapshot, InterfaceInventoryEntry,
};

use super::super::ManuallyAttached;
use super::PrnsNodeHandle;

/// How many frames a host lane holds in flight. RNS resource transfer bursts a whole window of parts at once (`Resource.WINDOW_MAX_FAST` is 75, plus its flexibility), so a lane carrying a transfer must be deeper than that window or it sheds parts and the transfer stalls; the old byte-budget collapsed a fat-MTU lane to a handful of slots, exactly that failure. Growable slots (`HeapFrameSlot`) cost only the frames actually in flight, so the depth is generous.
const HOST_LANE_DEPTH: usize = 256;

fn lane_depth_for(_slot_cap: usize) -> usize {
    HOST_LANE_DEPTH
}

#[derive(Clone)]
struct RuntimeIfac {
    context: IfacContext,
    network_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceAttachmentMetadata {
    pub name: Option<String>,
    pub origin: InterfaceOriginKind,
}

#[derive(Clone, Copy)]
struct InterfacePlacement {
    membership: Membership,
    origin: InterfaceOriginKind,
}

impl RuntimeIfac {
    fn snapshot(&self) -> InterfaceIfacSnapshot {
        InterfaceIfacSnapshot {
            signature: self.context.ifac_signature(),
            size: self.context.ifac_size(),
            network_name: self.network_name.clone(),
        }
    }
}

impl PrnsNodeHandle {
    fn next_attachment_epoch(&self) -> u64 {
        self.attachment_epochs.fetch_add(1, Ordering::Relaxed)
    }

    /// Attach an interface to the running node and get a handle to tear it back down. Grab any per-interface control handle (`.status()`, a radio's own controls) before calling this, since it takes the interface by value.
    ///
    /// `I: Send` is the host's bargain: the interface rides to the `run` task inside a `Send` builder closure which mints its run future there, so the future itself never has to be `Send` (what keeps `!Send` interface bodies legal) and the manifold stays `Send` and spawnable.
    pub fn add_interface<I>(&self, interface: I) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_with_metadata(
            interface,
            InterfaceAttachmentMetadata {
                name: None,
                origin: InterfaceOriginKind::Configured,
            },
        )
    }

    pub fn add_interface_with_ifac<I>(&self, interface: I, ifac: IfacContext) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_with_ifac_name(interface, ifac, None)
    }

    pub fn add_interface_with_ifac_name<I>(
        &self,
        interface: I,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_with_metadata_and_ifac_name(
            interface,
            InterfaceAttachmentMetadata {
                name: None,
                origin: InterfaceOriginKind::Configured,
            },
            ifac,
            network_name,
        )
    }

    pub fn add_interface_with_metadata<I>(
        &self,
        interface: I,
        metadata: InterfaceAttachmentMetadata,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_access(interface, metadata, None)
    }

    pub fn add_interface_with_metadata_and_ifac_name<I>(
        &self,
        interface: I,
        metadata: InterfaceAttachmentMetadata,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_interface_access(
            interface,
            metadata,
            Some(RuntimeIfac {
                context: ifac,
                network_name,
            }),
        )
    }

    fn add_interface_access<I>(
        &self,
        interface: I,
        metadata: InterfaceAttachmentMetadata,
        ifac: Option<RuntimeIfac>,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        let InterfaceAttachmentMetadata { name, origin } = metadata;
        let placement = InterfacePlacement {
            membership: Membership::Independent,
            origin,
        };
        let descriptor = interface.descriptor();
        let attachment_epoch = self.next_attachment_epoch();
        let view = interface.status_view();
        let connection = interface.connection_view();
        let frame_accounting = interface.frame_accounting_recorder();
        let attached = attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            InterfaceWiring {
                descriptor,
                placement,
                connection,
                frame_accounting,
                ifac: ifac.as_ref().map(|access| access.context.clone()),
            },
        );
        register_status(
            &self.interfaces,
            attached.id(),
            view.map(|view| RegisteredInterface {
                view,
                placement,
                descriptor: Some(descriptor),
                mode: descriptor.mode,
                gravity: descriptor.gravity,
                ifac: ifac.as_ref().map(RuntimeIfac::snapshot),
                name,
                rssi: None,
                byte_accounting: ByteAccounting::OwnTraffic,
                retired_member_bytes: RetiredMemberBytes::default(),
                retired_member_frame_accounting: RetiredMemberFrameAccounting::default(),
                attachment_epoch,
            }),
        );
        attached
    }

    #[must_use]
    pub fn interface_inventory(&self) -> std::vec::Vec<InterfaceInventoryEntry> {
        let Ok(map) = self.interfaces.lock() else {
            return std::vec::Vec::new();
        };
        map.iter()
            .flat_map(|(owner, registered)| {
                let owner = *owner;
                let placement = registered.placement;
                let ifac = registered.ifac.clone();
                let name = registered.name.clone();
                let byte_accounting = registered.byte_accounting;
                let retired = registered.retired_member_bytes;
                let retired_frame_accounting = registered.retired_member_frame_accounting;
                let attachment_epoch = registered.attachment_epoch;
                (registered.view)().into_iter().map(move |vitals| {
                    let counts = self.store.counts(vitals.id);
                    let (rx_bytes, tx_bytes) = if vitals.id == owner
                        && matches!(byte_accounting, ByteAccounting::FleetAggregate)
                    {
                        (retired.rx, retired.tx)
                    } else {
                        (vitals.rx_bytes, vitals.tx_bytes)
                    };
                    InterfaceInventoryEntry {
                        name: name.clone(),
                        origin: placement.origin,
                        attachment_epoch,
                        frame_accounting: if vitals.id == owner
                            && matches!(byte_accounting, ByteAccounting::FleetAggregate)
                        {
                            retired_frame_accounting.coverage()
                        } else {
                            vitals.frame_accounting.map_or(
                                FrameAccountingCoverage::Unavailable,
                                FrameAccountingCoverage::Complete,
                            )
                        },
                        snapshot: InterfaceSnapshot {
                            id: vitals.id,
                            mode: registered.mode,
                            gravity: registered.gravity,
                            connection: vitals.connection,
                            failure_reason: vitals.failure_reason,
                            rx_bytes,
                            tx_bytes,
                            transfer_rates: vitals.transfer_rates,
                            destinations: counts.destinations,
                            links: counts.links,
                            transported_links: counts.transported_links,
                            membership: placement.membership,
                        },
                        ifac: ifac.clone(),
                        rssi: registered.rssi,
                        members: std::vec::Vec::new(),
                    }
                })
            })
            .collect()
    }

    #[must_use]
    pub fn interface_timing_inventory(
        &self,
    ) -> std::vec::Vec<crate::node_introspection::InterfaceTimingSnapshot> {
        let Ok(map) = self.interfaces.lock() else {
            return std::vec::Vec::new();
        };
        map.values()
            .filter_map(|registered| {
                registered
                    .descriptor
                    .map(|descriptor| (descriptor, (registered.view)()))
            })
            .flat_map(|(descriptor, vitals)| {
                vitals.into_iter().map(move |vitals| {
                    crate::node_introspection::InterfaceTimingSnapshot {
                        id: vitals.id,
                        bitrate: descriptor.bitrate,
                        capabilities: descriptor.capabilities,
                        connection: vitals.connection,
                    }
                })
            })
            .collect()
    }

    #[must_use]
    pub fn set_interface_name(&self, id: InterfaceId, name: impl Into<String>) -> bool {
        let Ok(mut interfaces) = self.interfaces.lock() else {
            return false;
        };
        let Some(interface) = interfaces.get_mut(&id) else {
            return false;
        };
        interface.name = Some(name.into());
        true
    }

    /// Every interface attached through this handle, as a complete [`InterfaceSnapshot`]: live vitals read at call time joined with the engine counts and fleet position. The raw fleet an inspection face can project for its own presentation, with no app-side bookkeeping.
    #[must_use]
    pub fn interfaces(&self) -> std::vec::Vec<InterfaceSnapshot> {
        self.interface_inventory()
            .into_iter()
            .map(|entry| entry.snapshot)
            .collect()
    }

    /// Attach an interface supervisor: a node that owns no wire of its own but stands up a fleet member per validated connection through the [`Fleet`] handle it is given. The supervisor is no engine interface (no descriptor, no lanes); each member is an ordinary flat interface recorded under it, so teardown cascades to the whole fleet.
    pub fn supervise<S>(&self, supervisor: S) -> AttachedSupervisor
    where
        S: InterfaceSupervisor + ReportsStatus + Send + 'static,
    {
        self.supervise_access(supervisor, None)
    }

    pub fn supervise_with_ifac<S>(&self, supervisor: S, ifac: IfacContext) -> AttachedSupervisor
    where
        S: InterfaceSupervisor + ReportsStatus + Send + 'static,
    {
        self.supervise_with_ifac_name(supervisor, ifac, None)
    }

    pub fn supervise_with_ifac_name<S>(
        &self,
        supervisor: S,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> AttachedSupervisor
    where
        S: InterfaceSupervisor + ReportsStatus + Send + 'static,
    {
        self.supervise_access(
            supervisor,
            Some(RuntimeIfac {
                context: ifac,
                network_name,
            }),
        )
    }

    fn supervise_access<S>(&self, supervisor: S, ifac: Option<RuntimeIfac>) -> AttachedSupervisor
    where
        S: InterfaceSupervisor + ReportsStatus + Send + 'static,
    {
        let id = InterfaceId::from_channel_tag(S::KIND, supervisor.channel_tag());
        let placement = InterfacePlacement {
            membership: Membership::Independent,
            origin: InterfaceOriginKind::Configured,
        };
        let policy = supervisor.policy();
        let attachment_epoch = self.next_attachment_epoch();
        let view = supervisor.status_view();
        let ifac_status = ifac.as_ref().map(RuntimeIfac::snapshot);
        let fleet = Fleet {
            supervisor_id: id,
            commands: self.commands.clone(),
            iface_build: self.iface_build.clone(),
            notify_tx: self.notify_tx.clone(),
            interfaces: self.interfaces.clone(),
            attachment_epochs: self.attachment_epochs.clone(),
            ifac,
            entropy: self.entropy,
        };
        let build: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send> =
            Box::new(move || Box::pin(supervisor.run(fleet)));
        let _ = self.iface_build.send(DriverMsg::Add {
            id,
            supervisor: None,
            build,
        });
        register_status(
            &self.interfaces,
            id,
            view.map(|view| RegisteredInterface {
                view,
                placement,
                descriptor: None,
                mode: policy.mode,
                gravity: policy.gravity,
                ifac: ifac_status,
                name: None,
                rssi: None,
                byte_accounting: ByteAccounting::FleetAggregate,
                retired_member_bytes: RetiredMemberBytes::default(),
                retired_member_frame_accounting: RetiredMemberFrameAccounting::default(),
                attachment_epoch,
            }),
        );
        AttachedSupervisor {
            id,
            iface_build: self.iface_build.clone(),
        }
    }

    /// Detach the interface with this id (the inverse of [`add_interface`](Self::add_interface)): deregister its lanes on the manifold and stop its run future on the driver. For a supervisor, the driver cascades the stop to every member of its fleet. The routes learned through it stay warm for the departure grace, so a same-identity re-attach (a radio toggled off and on, a retune switched back) restores them; [`forget_interface`](Self::forget_interface) is the detach that drops them at once.
    pub fn remove_interface(&self, id: InterfaceId) {
        let _ = self.commands.send(HostCommand::RemoveInterface {
            id,
            departure: Departure::MayReturn,
        });
        let _ = self.iface_build.send(DriverMsg::Stop { id });
    }

    /// Detach like [`remove_interface`](Self::remove_interface) and drop the routes learned through the interface at once, instead of holding them warm for a return.
    pub fn forget_interface(&self, id: InterfaceId) {
        let _ = self.commands.send(HostCommand::RemoveInterface {
            id,
            departure: Departure::Forgotten,
        });
        let _ = self.iface_build.send(DriverMsg::Stop { id });
    }

    /// Attach anything from the interface menu and get back its kind's attachment handle — the one verb over [`add_interface`](Self::add_interface) and [`supervise`](Self::supervise).
    pub fn attach<A: Attachable>(&self, attachable: A) -> A::Attached {
        attachable.attach_to(self)
    }

    pub fn attach_with_ifac<A: Attachable>(&self, attachable: A, ifac: IfacContext) -> A::Attached {
        attachable.attach_to_with_ifac(self, ifac, None)
    }

    pub fn attach_with_ifac_name<A: Attachable>(
        &self,
        attachable: A,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> A::Attached {
        attachable.attach_to_with_ifac(self, ifac, network_name)
    }
}

/// One registration story per menu type: the type itself encodes whether it joins as a single wire (`add_interface`) or a discovery fleet (`supervise`), so no callsite has to know.
pub trait Attachable {
    type Attached;
    fn attach_to(self, handle: &PrnsNodeHandle) -> Self::Attached;
    fn attach_to_with_ifac(
        self,
        handle: &PrnsNodeHandle,
        ifac: IfacContext,
        network_name: Option<String>,
    ) -> Self::Attached;
}

/// The recipe's `interfaces` answer: [`ManuallyAttached`] says the app attaches through the handle itself, a closure over the handle is the inline shopping list, prefabs compose the common cases.
pub trait AttachIntent {
    fn attach(self, handle: &PrnsNodeHandle);
}

impl AttachIntent for ManuallyAttached {
    fn attach(self, _handle: &PrnsNodeHandle) {}
}

impl<F: FnOnce(&PrnsNodeHandle)> AttachIntent for F {
    fn attach(self, handle: &PrnsNodeHandle) {
        self(handle)
    }
}

/// A handle to one interface attached at runtime: its minted id and the lever to detach it. Dropping the handle leaves the interface running; only [`teardown`](Self::teardown) (or [`PrnsNodeHandle::remove_interface`]) takes it down.
pub struct AttachedInterface {
    id: InterfaceId,
    commands: UnboundedSender<HostCommand>,
    iface_build: UnboundedSender<DriverMsg>,
}

impl AttachedInterface {
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// Detach the interface: deregister its lanes on the manifold and stop its run future. Its routes stay warm for the departure grace, so a same-identity re-attach restores them.
    pub fn teardown(self) {
        let _ = self.commands.send(HostCommand::RemoveInterface {
            id: self.id,
            departure: Departure::MayReturn,
        });
        let _ = self.iface_build.send(DriverMsg::Stop { id: self.id });
    }
}

/// A handle to a supervisor attached through [`PrnsNodeHandle::supervise`]. Teardown is a single stop on the driver, ending its discovery loop and cascading to its whole fleet; dropping the handle leaves it running.
pub struct AttachedSupervisor {
    id: InterfaceId,
    iface_build: UnboundedSender<DriverMsg>,
}

impl AttachedSupervisor {
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// Detach the supervisor: stop its discovery loop and cascade teardown to its whole fleet.
    pub fn teardown(self) {
        let _ = self.iface_build.send(DriverMsg::Stop { id: self.id });
    }
}

/// Wire one interface onto the running node: build its grant lanes + seam, hand the manifold the `Send` lane halves, and hand the driver the `Send` builder that mints its run future. `supervisor` records it as a fleet member so the driver cascades teardown.
struct InterfaceWiring {
    descriptor: crate::interfaces::InterfaceDescriptor,
    placement: InterfacePlacement,
    connection: Option<ConnectionView>,
    frame_accounting: Option<FrameAccountingRecorder>,
    ifac: Option<IfacContext>,
}

fn attach_interface<I>(
    commands: &UnboundedSender<HostCommand>,
    iface_build: &UnboundedSender<DriverMsg>,
    notify_tx: &UnboundedSender<InterfaceId>,
    interface: I,
    wiring: InterfaceWiring,
) -> AttachedInterface
where
    I: Interface + Send + 'static,
{
    let InterfaceWiring {
        descriptor,
        placement,
        connection,
        frame_accounting,
        ifac,
    } = wiring;
    let id = descriptor.id;
    let supervisor = match placement.membership {
        Membership::Independent => None,
        Membership::FleetMember { supervisor_id } => Some(supervisor_id),
    };
    let logical_interface = supervisor.unwrap_or(id);
    let slot_cap = frame_cap_for(&descriptor);
    let depth = lane_depth_for(slot_cap);
    let (in_producer, in_consumer) = tokio_grant_lane(slot_cap, depth);
    let (out_producer, out_consumer) = tokio_grant_lane(slot_cap, depth);
    let seam = TokioInterfaceSeam::new(id, in_producer, notify_tx.clone(), out_consumer)
        .with_origin(placement.origin)
        .with_commands(commands.clone());
    let build: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send> =
        Box::new(move || Box::pin(interface.run(seam)));
    let _ = commands.send(HostCommand::AddInterface(AddInterfaceCommand {
        descriptor,
        logical_interface,
        inbound: in_consumer,
        egress: out_producer,
        connection,
        frame_accounting,
        ifac,
    }));
    let _ = iface_build.send(DriverMsg::Add {
        id,
        supervisor,
        build,
    });
    AttachedInterface {
        id,
        commands: commands.clone(),
        iface_build: iface_build.clone(),
    }
}

/// A supervisor's lever to stand up fleet members. Each [`add`](Self::add) registers a flat engine interface recorded as this supervisor's member; the supervisor typically holds the returned [`AttachedInterface`] to detach that member when its link drops.
pub struct Fleet {
    supervisor_id: InterfaceId,
    commands: UnboundedSender<HostCommand>,
    iface_build: UnboundedSender<DriverMsg>,
    notify_tx: UnboundedSender<InterfaceId>,
    interfaces: Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    attachment_epochs: Arc<AtomicU64>,
    ifac: Option<RuntimeIfac>,
    entropy: crate::manifold::driver::TokioEntropy,
}

impl Fleet {
    pub fn fill_entropy(&self, bytes: &mut [u8]) {
        self.entropy.fill(bytes);
    }

    /// Stand up a fleet member under this supervisor — identical to [`PrnsNodeHandle::add_interface`] except the member is recorded as this supervisor's, so a supervisor teardown takes it with it.
    pub fn add<I>(&self, interface: I) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_with_peer_status(interface, None, None)
    }

    /// Stand up a named fleet member, optionally recording link-up RSSI for status nesting.
    pub fn add_named<I>(
        &self,
        interface: I,
        name: impl Into<String>,
        rssi: Option<i8>,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        self.add_with_peer_status(interface, Some(name.into()), rssi)
    }

    fn add_with_peer_status<I>(
        &self,
        interface: I,
        name: Option<String>,
        rssi: Option<i8>,
    ) -> AttachedInterface
    where
        I: Interface + ReportsStatus + Send + 'static,
    {
        let view = interface.status_view();
        let connection = interface.connection_view();
        let frame_accounting = interface.frame_accounting_recorder();
        let descriptor = interface.descriptor();
        let attachment_epoch = self.attachment_epochs.fetch_add(1, Ordering::Relaxed);
        let placement = InterfacePlacement {
            membership: Membership::FleetMember {
                supervisor_id: self.supervisor_id,
            },
            origin: InterfaceOriginKind::Configured,
        };
        let attached = attach_interface(
            &self.commands,
            &self.iface_build,
            &self.notify_tx,
            interface,
            InterfaceWiring {
                descriptor,
                placement,
                connection,
                frame_accounting,
                ifac: self.ifac.as_ref().map(|access| access.context.clone()),
            },
        );
        register_status(
            &self.interfaces,
            attached.id(),
            view.map(|view| RegisteredInterface {
                view,
                placement,
                descriptor: Some(descriptor),
                mode: descriptor.mode,
                gravity: descriptor.gravity,
                ifac: self.ifac.as_ref().map(RuntimeIfac::snapshot),
                name,
                rssi,
                byte_accounting: ByteAccounting::OwnTraffic,
                retired_member_bytes: RetiredMemberBytes::default(),
                retired_member_frame_accounting: RetiredMemberFrameAccounting::default(),
                attachment_epoch,
            }),
        );
        attached
    }

    /// A [`Fleet`] wired to no manifold: member builds and host commands flow into the returned [`DetachedFleet`] tail and go nowhere. For driving a supervisor by hand (unit tests, a bench harness).
    #[must_use]
    pub fn detached(supervisor_id: InterfaceId) -> (Self, DetachedFleet) {
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let (iface_build, iface_build_rx) = mpsc::unbounded_channel();
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let fleet = Fleet {
            supervisor_id,
            commands,
            iface_build,
            notify_tx,
            interfaces: Arc::new(Mutex::new(HashMap::new())),
            attachment_epochs: Arc::new(AtomicU64::new(0)),
            ifac: None,
            entropy: crate::manifold::driver::TokioEntropy,
        };
        let tail = DetachedFleet {
            _commands: commands_rx,
            _iface_build: iface_build_rx,
            _notify: notify_rx,
        };
        (fleet, tail)
    }
}

/// The unplugged end of [`Fleet::detached`]: holds the channel tails so the fleet's sends stay deliverable while a hand-driven harness runs. Drop it and sends start failing, like a runtime whose manifold exited.
pub struct DetachedFleet {
    _commands: UnboundedReceiver<HostCommand>,
    _iface_build: UnboundedReceiver<DriverMsg>,
    _notify: UnboundedReceiver<InterfaceId>,
}

/// An interface supervisor: a node that owns no wire of its own but runs a discovery loop and stands up a fleet member per validated connection. Attached with [`PrnsNodeHandle::supervise`].
#[allow(async_fn_in_trait)]
pub trait InterfaceSupervisor {
    /// The medium this supervisor stands for — the namespace root of its id.
    const KIND: InterfaceKind;

    /// The bytes that uniquely tag this supervisor, typically config-derived (the group it serves); the same rules as [`channel_tag`](crate::manifold::interface_seam::Interface::channel_tag) apply.
    fn channel_tag(&self) -> &[u8];

    fn policy(&self) -> crate::interfaces::EffectiveInterfacePolicy;

    async fn run(self, fleet: Fleet);
}

/// A message to the interface driver: a new interface to start driving, or a request to stop one. The driver lives on the `!Send` `run` task, so an interface's `!Send` run future never has to cross a thread — only the `Send` builder closure does.
pub(super) enum DriverMsg {
    Add {
        id: InterfaceId,
        supervisor: Option<InterfaceId>,
        build: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send>,
    },
    Stop {
        id: InterfaceId,
    },
}

/// Drive every interface run future — the recipe's initial set, plus any added through the handle at runtime — on the `run` task. Each runtime-added interface is wrapped with a stop signal so [`PrnsNodeHandle::remove_interface`] can drop it mid-flight; the initial set runs for the node's life.
pub(super) async fn drive_interfaces(
    initial: std::vec::Vec<Pin<Box<dyn Future<Output = ()>>>>,
    mut messages: UnboundedReceiver<DriverMsg>,
    commands: UnboundedSender<HostCommand>,
    interfaces: Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
) {
    let mut futures: FuturesUnordered<Pin<Box<dyn Future<Output = Option<InterfaceId>>>>> = initial
        .into_iter()
        .map(
            |run| -> Pin<Box<dyn Future<Output = Option<InterfaceId>>>> {
                Box::pin(async move {
                    run.await;
                    None
                })
            },
        )
        .collect();
    let mut stops: HashMap<InterfaceId, oneshot::Sender<()>> = HashMap::new();
    let mut supervisor_of: HashMap<InterfaceId, InterfaceId> = HashMap::new();
    let mut open = true;
    loop {
        if !open && futures.is_empty() {
            return;
        }
        tokio::select! {
            message = messages.recv(), if open => match message {
                Some(DriverMsg::Add { id, supervisor, build }) => {
                    if let Some(supervisor_id) = supervisor {
                        let _ = supervisor_of.insert(id, supervisor_id);
                    }
                    let (stop_tx, stop_rx) = oneshot::channel();
                    let run = std::panic::catch_unwind(AssertUnwindSafe(build));
                    let guarded: Pin<Box<dyn Future<Output = Option<InterfaceId>>>> = match run {
                        Ok(run) => Box::pin(async move {
                            tokio::select! {
                                _ = AssertUnwindSafe(run).catch_unwind() => {}
                                _ = stop_rx => {}
                            }
                            Some(id)
                        }),
                        Err(_) => Box::pin(async move { Some(id) }),
                    };
                    futures.push(guarded);
                    stops.insert(id, stop_tx);
                }
                Some(DriverMsg::Stop { id }) => {
                    let stopped = stop_interface(&mut stops, id);
                    match supervisor_of.remove(&id) {
                        Some(supervisor) => retire_member_status(&interfaces, id, supervisor),
                        None => forget_status(&interfaces, id),
                    }
                    stop_supervised_members(
                        &mut stops,
                        &mut supervisor_of,
                        &interfaces,
                        &commands,
                        id,
                    );
                    if stopped {
                        drain_stopped_interface(
                            &mut futures,
                            &mut stops,
                            &mut supervisor_of,
                            &interfaces,
                            &commands,
                            id,
                        )
                        .await;
                    }
                }
                None => open = false,
            },
            // An interface whose run future ended on its own (a dropped connection, no reconnect) deregisters itself: its descriptor must not outlive its wire. A future ended by a `Stop` already had its id pulled from `stops`, so the `stops.remove` here is what distinguishes a natural completion from a deliberate one.
            done = futures.next(), if !futures.is_empty() => {
                if let Some(Some(id)) = done {
                    complete_interface(
                        &mut stops,
                        &mut supervisor_of,
                        &interfaces,
                        &commands,
                        id,
                    );
                }
            }
        }
    }
}

/// A status view the runtime tracks centrally, tagged with where its interface sits in the fleet. `interfaces()` joins each with the engine's count store to mint an `InterfaceSnapshot`.
pub(super) struct RegisteredInterface {
    view: StatusView,
    placement: InterfacePlacement,
    descriptor: Option<InterfaceDescriptor>,
    mode: crate::interfaces::InterfaceMode,
    gravity: crate::interfaces::InterfaceGravity,
    ifac: Option<InterfaceIfacSnapshot>,
    name: Option<String>,
    rssi: Option<i8>,
    byte_accounting: ByteAccounting,
    retired_member_bytes: RetiredMemberBytes,
    retired_member_frame_accounting: RetiredMemberFrameAccounting,
    attachment_epoch: u64,
}

/// Whether this status owns its byte counters or mirrors the members that inventory tracks separately.
#[derive(Clone, Copy)]
pub(super) enum ByteAccounting {
    OwnTraffic,
    FleetAggregate,
}

/// Byte totals carried over from fleet members that have departed, so a supervisor's traffic odometer stays monotonic across member churn instead of dropping each departed connection's bytes from the aggregate.
#[derive(Clone, Copy, Default)]
pub(super) struct RetiredMemberBytes {
    rx: u64,
    tx: u64,
}

#[derive(Clone, Copy, Default)]
enum RetiredMemberFrameAccounting {
    #[default]
    Unseen,
    Complete(FrameAccounting),
    Incomplete,
}

impl RetiredMemberFrameAccounting {
    fn coverage(self) -> FrameAccountingCoverage {
        match self {
            Self::Unseen => FrameAccountingCoverage::Unavailable,
            Self::Complete(accounting) => FrameAccountingCoverage::Complete(accounting),
            Self::Incomplete => FrameAccountingCoverage::Incomplete,
        }
    }

    fn include(&mut self, retired: Self) {
        *self = match (*self, retired) {
            (Self::Incomplete, _) | (_, Self::Incomplete) => Self::Incomplete,
            (Self::Unseen, other) | (other, Self::Unseen) => other,
            (Self::Complete(left), Self::Complete(right)) => {
                Self::Complete(left.saturating_add(right))
            }
        };
    }
}

fn register_status(
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    id: InterfaceId,
    registered: Option<RegisteredInterface>,
) {
    if let (Some(registered), Ok(mut map)) = (registered, interfaces.lock()) {
        map.insert(id, registered);
    }
}

fn forget_status(
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    id: InterfaceId,
) {
    if let Ok(mut map) = interfaces.lock() {
        map.remove(&id);
    }
}

fn retire_member_status(
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    member: InterfaceId,
    supervisor: InterfaceId,
) {
    let Ok(mut map) = interfaces.lock() else {
        return;
    };
    let Some(departed) = map.remove(&member) else {
        return;
    };
    let vitals = (departed.view)();
    let mut retired_frames = if vitals.is_empty() {
        RetiredMemberFrameAccounting::Incomplete
    } else {
        RetiredMemberFrameAccounting::Unseen
    };
    let (rx, tx) = vitals.into_iter().fold((0u64, 0u64), |(rx, tx), vitals| {
        retired_frames.include(match vitals.frame_accounting {
            Some(accounting) => RetiredMemberFrameAccounting::Complete(accounting),
            None => RetiredMemberFrameAccounting::Incomplete,
        });
        (
            rx.saturating_add(vitals.rx_bytes),
            tx.saturating_add(vitals.tx_bytes),
        )
    });
    let Some(kept) = map.get_mut(&supervisor) else {
        return;
    };
    kept.retired_member_bytes.rx = kept.retired_member_bytes.rx.saturating_add(rx);
    kept.retired_member_bytes.tx = kept.retired_member_bytes.tx.saturating_add(tx);
    kept.retired_member_frame_accounting.include(retired_frames);
}

fn stop_interface(stops: &mut HashMap<InterfaceId, oneshot::Sender<()>>, id: InterfaceId) -> bool {
    if let Some(stop) = stops.remove(&id) {
        let _ = stop.send(());
        true
    } else {
        false
    }
}

async fn drain_stopped_interface(
    futures: &mut FuturesUnordered<Pin<Box<dyn Future<Output = Option<InterfaceId>>>>>,
    stops: &mut HashMap<InterfaceId, oneshot::Sender<()>>,
    supervisor_of: &mut HashMap<InterfaceId, InterfaceId>,
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    commands: &UnboundedSender<HostCommand>,
    stopped_id: InterfaceId,
) {
    while let Some(done) = futures.next().await {
        let Some(id) = done else {
            continue;
        };
        complete_interface(stops, supervisor_of, interfaces, commands, id);
        if id == stopped_id {
            return;
        }
    }
}

fn complete_interface(
    stops: &mut HashMap<InterfaceId, oneshot::Sender<()>>,
    supervisor_of: &mut HashMap<InterfaceId, InterfaceId>,
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    commands: &UnboundedSender<HostCommand>,
    id: InterfaceId,
) {
    if stops.remove(&id).is_some() {
        match supervisor_of.remove(&id) {
            Some(supervisor) => retire_member_status(interfaces, id, supervisor),
            None => forget_status(interfaces, id),
        }
        let _ = commands.send(HostCommand::RemoveInterface {
            id,
            departure: Departure::MayReturn,
        });
        stop_supervised_members(stops, supervisor_of, interfaces, commands, id);
    }
}

fn stop_supervised_members(
    stops: &mut HashMap<InterfaceId, oneshot::Sender<()>>,
    supervisor_of: &mut HashMap<InterfaceId, InterfaceId>,
    interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
    commands: &UnboundedSender<HostCommand>,
    supervisor_id: InterfaceId,
) {
    let members: std::vec::Vec<InterfaceId> = supervisor_of
        .iter()
        .filter(|(_, supervisor)| **supervisor == supervisor_id)
        .map(|(member, _)| *member)
        .collect();
    for member in members {
        let _ = stop_interface(stops, member);
        supervisor_of.remove(&member);
        forget_status(interfaces, member);
        let _ = commands.send(HostCommand::RemoveInterface {
            id: member,
            departure: Departure::MayReturn,
        });
    }
}

#[cfg(test)]
mod tests;
