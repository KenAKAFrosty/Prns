use prns_core::interfaces::{AttachedInterfaces, IndexedAttachedInterfaces};

use crate::engine::{Departure, EngineState, InstantMillis};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{FrameAccountingRecorder, InterfaceDescriptor, InterfaceId};
use crate::manifold::interface_seam::{frame_cap_for, BROADCAST_WIRE_FRAME_LEN};
use crate::manifold::Host;
use crate::storage::StorageLayout;

use super::egress::{Egress, InterfacePacer};
use super::host_protocol::AddInterfaceCommand;
use super::TokioGrantConsumer;

pub(super) struct InterfaceTopology {
    pub(super) interfaces: IndexedAttachedInterfaces,
    pub(super) ifacs: std::vec::Vec<InterfaceIfac>,
    pub(super) inbound_lanes: std::vec::Vec<(InterfaceId, TokioGrantConsumer)>,
    frame_accounting: std::vec::Vec<FrameAccountingRecorder>,
    pub(super) pacers: std::vec::Vec<InterfacePacer>,
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
        for descriptor in interfaces.descriptors() {
            #[cfg(feature = "runtime-metrics")]
            engine.attach_metrics_interface(descriptor.id, descriptor.id);
            engine.interface_attached(descriptor.id, host.now());
        }
        let pacers = interfaces
            .descriptors()
            .iter()
            .map(|descriptor| InterfacePacer::from_descriptor(descriptor, descriptor.id))
            .collect();
        Self {
            interfaces,
            ifacs,
            inbound_lanes,
            frame_accounting: std::vec::Vec::new(),
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
        self.pacers.push(InterfacePacer::from_descriptor(
            &descriptor,
            logical_interface,
        ));
        #[cfg(feature = "runtime-metrics")]
        engine.attach_metrics_interface(id, logical_interface);
        engine.interface_attached(id, now);
        self.interfaces.push(descriptor);
        self.inbound_lanes.push((id, inbound));
        if let Some(recorder) = frame_accounting {
            debug_assert_eq!(recorder.id(), id);
            if recorder.id() == id {
                self.frame_accounting.push(recorder);
            }
        }
        self.egress
            .add_lane(id, logical_interface, egress, connection);
        if let Some(context) = ifac {
            self.ifacs.push(InterfaceIfac { id, context });
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
        self.inbound_lanes.retain(|(lane_id, _)| *lane_id != id);
        self.frame_accounting.retain(|recorder| recorder.id() != id);
        self.pacers.retain(|pacer| pacer.id != id);
        self.ifacs.retain(|entry| entry.id != id);
        self.egress.remove_lane(id);
    }

    pub(super) fn frame_accounting_recorder(
        &self,
        source: InterfaceId,
    ) -> Option<FrameAccountingRecorder> {
        self.frame_accounting
            .iter()
            .find(|recorder| recorder.id() == source)
            .cloned()
    }
}
