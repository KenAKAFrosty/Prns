use crate::engine::directives::{EngineDirective, EngineDirectives};
use alloc::vec::Vec;

#[derive(Debug, Default)]
pub struct HeapEngineDirectives {
    directives: Vec<EngineDirective>,
}

impl EngineDirectives for HeapEngineDirectives {
    fn clear(&mut self) {
        self.directives.clear();
    }
    fn push(&mut self, directive: EngineDirective) {
        self.directives.push(directive);
    }
    fn iter(&self) -> core::slice::Iter<'_, EngineDirective> {
        self.directives.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MAX_REGISTERED_INTERFACES;
    use crate::interfaces::InterfaceId;
    use crate::wire::DestinationHash;
    use heapless::Vec as HeaplessVec;

    fn directive(byte: u8) -> EngineDirective {
        let mut fire_on: HeaplessVec<InterfaceId, MAX_REGISTERED_INTERFACES> = HeaplessVec::new();
        let _ = fire_on.push(InterfaceId::new([byte; 16]));
        EngineDirective::ReemitAnnounce {
            destination: DestinationHash::new([byte; 16]),
            fire_on,
        }
    }

    #[test]
    fn grows_past_a_fixed_cap_and_clears() {
        let mut directives = HeapEngineDirectives::default();
        for n in 0..500u32 {
            directives.push(directive(n as u8));
        }
        assert_eq!(directives.len(), 500);
        directives.clear();
        assert!(directives.is_empty());
    }
}
