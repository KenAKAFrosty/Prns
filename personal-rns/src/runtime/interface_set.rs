use crate::reactor::interface_seam::Interface;

pub trait InterfaceAttach {
    fn attach<I: Interface>(&mut self, interface: I);
}

pub trait InterfaceSet {
    fn attach_all<A: InterfaceAttach>(self, attach: &mut A);
}

impl InterfaceSet for () {
    fn attach_all<A: InterfaceAttach>(self, _attach: &mut A) {}
}

macro_rules! interface_set_tuple {
    ($($name:ident),+) => {
        impl<$($name: Interface),+> InterfaceSet for ($($name,)+) {
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
