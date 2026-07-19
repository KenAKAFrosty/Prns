use crate::interfaces::InterfaceId;
use crate::units::InstantMillis;

pub trait RouteWarmth {
    fn warm_until(&self, interface: InterfaceId) -> Option<InstantMillis>;
}

impl RouteWarmth for () {
    fn warm_until(&self, _interface: InterfaceId) -> Option<InstantMillis> {
        None
    }
}

pub struct WarmestOf<'a>(pub &'a dyn RouteWarmth, pub &'a dyn RouteWarmth);

impl RouteWarmth for WarmestOf<'_> {
    fn warm_until(&self, interface: InterfaceId) -> Option<InstantMillis> {
        match (self.0.warm_until(interface), self.1.warm_until(interface)) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}
