mod impls;

pub use impls::*;

use crate::interfaces::InterfaceId;
use crate::interfaces::MAX_REGISTERED_INTERFACES;
use crate::wire::DestinationHash;
use heapless::Vec;

#[derive(Debug, Clone)]
pub enum EngineDirective {
    ReemitAnnounce {
        destination: DestinationHash,
        fire_on: Vec<InterfaceId, MAX_REGISTERED_INTERFACES>,
    },
}

pub trait EngineDirectives {
    fn clear(&mut self);
    fn push(&mut self, directive: EngineDirective);
    fn iter(&self) -> core::slice::Iter<'_, EngineDirective>;

    fn len(&self) -> usize {
        self.iter().len()
    }
    fn is_empty(&self) -> bool {
        self.iter().len() == 0
    }
}
