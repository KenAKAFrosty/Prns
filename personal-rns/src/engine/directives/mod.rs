//! Engine directives — a tick's outcomes as data.
//!
//! A [`tick`](crate::engine::tick) produces [`EngineDirective`]s: what to do, and
//! (the engine's decision, its predicates already applied) which interfaces to do
//! it on. The runtime reads them through the `TickOutput` window and delivers each
//! to its live interfaces — the engine is the brain that decides, the runtime the
//! muscle that delivers.

mod impls;

pub use impls::*;

use crate::interfaces::MAX_REGISTERED_INTERFACES;
use crate::interfaces::InterfaceId;
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
