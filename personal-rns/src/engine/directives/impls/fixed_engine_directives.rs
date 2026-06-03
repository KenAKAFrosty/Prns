use crate::engine::directives::{EngineDirective, EngineDirectives};

#[derive(Debug, Default)]
pub struct FixedEngineDirectives<const MAX_DIRECTIVES: usize> {
    directives: heapless::Vec<EngineDirective, MAX_DIRECTIVES>,
}

impl<const MAX_DIRECTIVES: usize> EngineDirectives for FixedEngineDirectives<MAX_DIRECTIVES> {
    fn clear(&mut self) {
        self.directives.clear();
    }
    fn push(&mut self, directive: EngineDirective) {
        // Sized to the routing table, so due directives never exceed capacity.
        let _ = self.directives.push(directive);
    }
    fn as_slice(&self) -> &[EngineDirective] {
        &self.directives
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MAX_REGISTERED_INTERFACES;
    use crate::interfaces::InterfaceId;
    use crate::wire::DestinationHash;
    use heapless::Vec;

    fn directive(byte: u8) -> EngineDirective {
        let mut fire_on: Vec<InterfaceId, MAX_REGISTERED_INTERFACES> = Vec::new();
        let _ = fire_on.push(InterfaceId::new([byte; 16]));
        EngineDirective::ReemitAnnounce {
            destination: DestinationHash::new([byte; 16]),
            fire_on,
        }
    }

    #[test]
    fn fills_clears_and_exposes() {
        let mut directives = FixedEngineDirectives::<4>::default();
        assert!(directives.is_empty());
        directives.push(directive(1));
        directives.push(directive(2));
        assert_eq!(directives.len(), 2);
        assert_eq!(directives.as_slice().len(), 2);
        directives.clear();
        assert!(directives.is_empty());
    }
}
