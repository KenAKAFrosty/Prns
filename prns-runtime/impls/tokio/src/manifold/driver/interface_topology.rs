use prns_core::interfaces::{AttachedInterfaces, IndexedAttachedInterfaces};
use prns_core::lemire_index::IndexRow;

use crate::engine::{Departure, EngineState, InstantMillis};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{FrameAccountingRecorder, InterfaceDescriptor, InterfaceId};
use crate::manifold::interface_seam::{frame_cap_for, BROADCAST_WIRE_FRAME_LEN};
use crate::manifold::Host;
use crate::storage::StorageLayout;

use super::egress::{Egress, InterfaceIfacs, InterfacePacer, InterfacePacers};
use super::host_protocol::AddInterfaceCommand;
use super::indexed_rows::IndexedRows;
use super::{HeapFrameSlot, TokioGrantConsumer};

pub(super) struct InboundLane {
    pub(super) id: InterfaceId,
    pub(super) consumer: TokioGrantConsumer,
}

impl IndexRow for InboundLane {
    type Key = InterfaceId;

    fn index_key(&self) -> &Self::Key {
        &self.id
    }
}

struct FrameAccountingLane {
    id: InterfaceId,
    recorder: FrameAccountingRecorder,
}

impl IndexRow for FrameAccountingLane {
    type Key = InterfaceId;

    fn index_key(&self) -> &Self::Key {
        &self.id
    }
}

pub(super) struct InterfaceTopology {
    pub(super) interfaces: IndexedAttachedInterfaces,
    pub(super) ifacs: InterfaceIfacs,
    pub(super) inbound_lanes: IndexedRows<InboundLane>,
    frame_accounting: IndexedRows<FrameAccountingLane>,
    pub(super) pacers: InterfacePacers,
    pub(super) egress: Egress,
}

impl InterfaceTopology {
    pub(super) fn new<S: StorageLayout, H: Host>(
        descriptors: std::vec::Vec<InterfaceDescriptor>,
        ifacs: std::vec::Vec<InterfaceIfac>,
        inbound_lanes: std::vec::Vec<(InterfaceId, TokioGrantConsumer)>,
        egress: Egress,
        engine: &mut EngineState<S>,
        host: &H,
    ) -> Self {
        let interfaces = IndexedAttachedInterfaces::from(descriptors);
        let inbound_lanes = inbound_lanes
            .into_iter()
            .map(|(id, consumer)| InboundLane { id, consumer })
            .collect::<std::vec::Vec<_>>()
            .into();
        for descriptor in interfaces.descriptors() {
            #[cfg(feature = "runtime-metrics")]
            engine.attach_metrics_interface(descriptor.id, descriptor.id);
            engine.interface_attached(descriptor.id, host.now());
        }
        let pacers = interfaces
            .descriptors()
            .iter()
            .map(|descriptor| InterfacePacer::from_descriptor(descriptor, descriptor.id))
            .collect::<std::vec::Vec<_>>()
            .into();
        Self {
            interfaces,
            ifacs: ifacs.into(),
            inbound_lanes,
            frame_accounting: IndexedRows::default(),
            pacers,
            egress,
        }
    }

    pub(super) fn view(&self) -> AttachedInterfaces<'_> {
        self.interfaces.view()
    }

    pub(super) fn frame_cap(&self) -> usize {
        self.interfaces
            .descriptors()
            .iter()
            .map(frame_cap_for)
            .max()
            .unwrap_or(BROADCAST_WIRE_FRAME_LEN)
    }

    pub(super) fn attach<S: StorageLayout>(
        &mut self,
        engine: &mut EngineState<S>,
        add: AddInterfaceCommand,
        now: InstantMillis,
    ) -> Option<(InterfaceId, usize)> {
        let AddInterfaceCommand {
            descriptor,
            logical_interface,
            inbound,
            egress,
            connection,
            frame_accounting,
            ifac,
        } = add;
        let id = descriptor.id;
        if self.view().descriptor_for(id).is_some() {
            debug_assert!(
                false,
                "interface id collision (kind byte {}): two live channels produced the same channel tag — an interface returned a non-unique channel_tag",
                id.as_bytes()[0],
            );
            drop((inbound, egress));
            return None;
        }

        let frame_cap = frame_cap_for(&descriptor);
        let pacer_inserted = self.pacers.push(InterfacePacer::from_descriptor(
            &descriptor,
            logical_interface,
        ));
        debug_assert!(
            pacer_inserted,
            "pacer rows require unique live interface ids"
        );
        #[cfg(feature = "runtime-metrics")]
        engine.attach_metrics_interface(id, logical_interface);
        engine.interface_attached(id, now);
        self.interfaces.push(descriptor);
        let inbound_inserted = self.inbound_lanes.push(InboundLane {
            id,
            consumer: inbound,
        });
        debug_assert!(
            inbound_inserted,
            "inbound lanes require unique live interface ids"
        );
        if let Some(recorder) = frame_accounting {
            debug_assert_eq!(recorder.id(), id);
            if recorder.id() == id {
                let accounting_inserted = self
                    .frame_accounting
                    .push(FrameAccountingLane { id, recorder });
                debug_assert!(
                    accounting_inserted,
                    "frame-accounting rows require unique live interface ids"
                );
            }
        }
        self.egress
            .add_lane(id, logical_interface, egress, connection);
        if let Some(context) = ifac {
            let ifac_inserted = self.ifacs.push(InterfaceIfac { id, context });
            debug_assert!(ifac_inserted, "IFAC rows require unique live interface ids");
        }
        Some((id, frame_cap))
    }

    pub(super) fn detach<S: StorageLayout>(
        &mut self,
        engine: &mut EngineState<S>,
        id: InterfaceId,
        departure: Departure,
        now: InstantMillis,
    ) {
        engine.interface_departed(id, departure, now);
        self.interfaces.remove(id);
        self.inbound_lanes.remove(&id);
        self.frame_accounting.remove(&id);
        self.pacers.remove(&id);
        self.ifacs.remove(id);
        self.egress.remove_lane(id);
    }

    pub(super) fn frame_accounting_recorder(
        &self,
        source: InterfaceId,
    ) -> Option<FrameAccountingRecorder> {
        self.frame_accounting
            .get(&source)
            .map(|entry| entry.recorder.clone())
    }

    pub(super) fn return_inbound_slot(&mut self, source: InterfaceId, slot: HeapFrameSlot) {
        if let Some(lane) = self.inbound_lanes.get_mut(&source) {
            lane.consumer.return_slot(slot);
        }
    }
}
