use crate::interfaces::{AttachedInterfaces, InterfaceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressTarget {
    AllInterfaces,
    Interface(InterfaceId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressTargetRejection {
    UnknownInterface,
    InterfaceCannotTransmit,
}

impl EgressTarget {
    pub(crate) fn admit(
        self,
        interfaces: AttachedInterfaces<'_>,
    ) -> Result<(), EgressTargetRejection> {
        let Self::Interface(interface) = self else {
            return Ok(());
        };
        let Some(descriptor) = interfaces.descriptor_for(interface) else {
            return Err(EgressTargetRejection::UnknownInterface);
        };
        if !descriptor.capabilities.allows_transmit() {
            return Err(EgressTargetRejection::InterfaceCannotTransmit);
        }
        Ok(())
    }
}
