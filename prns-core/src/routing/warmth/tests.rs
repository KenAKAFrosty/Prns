use super::{RouteWarmth, WarmestOf};
use crate::interfaces::InterfaceId;
use crate::units::InstantMillis;

#[test]
fn the_warmest_of_two_sources_wins() {
    struct At(u64);
    impl RouteWarmth for At {
        fn warm_until(&self, _interface: InterfaceId) -> Option<InstantMillis> {
            Some(InstantMillis(self.0))
        }
    }
    let interface = InterfaceId::new([1; 8]);
    assert_eq!(
        WarmestOf(&At(5_000), &At(9_000)).warm_until(interface),
        Some(InstantMillis(9_000)),
    );
    assert_eq!(
        WarmestOf(&(), &At(9_000)).warm_until(interface),
        Some(InstantMillis(9_000)),
    );
    assert_eq!(WarmestOf(&(), &()).warm_until(interface), None);
}
