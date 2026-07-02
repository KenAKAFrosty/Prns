use crate::reactor::interface_seam::Interface;

pub trait InterfaceAttach {
    fn attach<I: Interface + 'static>(&mut self, interface: I);
}

pub trait InterfaceSet {
    fn attach_all<A: InterfaceAttach>(self, attach: &mut A);
}

impl InterfaceSet for () {
    fn attach_all<A: InterfaceAttach>(self, _attach: &mut A) {}
}

/// A run-time-sized set of one interface kind — what a fan-in responder (N like listeners) or any
/// caller that builds its wires in a loop hands the recipe. Heterogeneous sets still take the tuple
/// form; this is the homogeneous escape hatch, so it needs an owning heap and rides the `std` lane.
#[cfg(feature = "std")]
impl<I: Interface + 'static> InterfaceSet for std::vec::Vec<I> {
    fn attach_all<A: InterfaceAttach>(self, attach: &mut A) {
        for interface in self {
            attach.attach(interface);
        }
    }
}

macro_rules! interface_set_tuple {
    ($($name:ident),+) => {
        impl<$($name: Interface + 'static),+> InterfaceSet for ($($name,)+) {
            #[allow(non_snake_case)]
            fn attach_all<A: InterfaceAttach>(self, attach: &mut A) {
                let ($($name,)+) = self;
                $(attach.attach($name);)+
            }
        }
    };
}

interface_set_tuple!(I0);
interface_set_tuple!(I0, I1);
interface_set_tuple!(I0, I1, I2);
interface_set_tuple!(I0, I1, I2, I3);
interface_set_tuple!(I0, I1, I2, I3, I4);
interface_set_tuple!(I0, I1, I2, I3, I4, I5);
interface_set_tuple!(I0, I1, I2, I3, I4, I5, I6);
interface_set_tuple!(I0, I1, I2, I3, I4, I5, I6, I7);

#[macro_export]
macro_rules! interfaces {
    ($($iface:expr),* $(,)?) => {
        ($($iface,)*)
    };
}
