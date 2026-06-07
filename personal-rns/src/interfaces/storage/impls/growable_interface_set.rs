use alloc::vec::Vec;

use crate::interfaces::storage::InterfaceSet;

#[derive(Debug)]
pub struct GrowableInterfaceSet<H> {
    interfaces: Vec<H>,
}

impl<H> GrowableInterfaceSet<H> {
    pub const fn new() -> Self {
        Self {
            interfaces: Vec::new(),
        }
    }
}

impl<H> Default for GrowableInterfaceSet<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> InterfaceSet for GrowableInterfaceSet<H> {
    type Item = H;

    fn len(&self) -> usize {
        self.interfaces.len()
    }

    fn push(&mut self, interface: H) -> Result<(), H> {
        self.interfaces.push(interface);
        Ok(())
    }

    fn remove(&mut self, index: usize) -> H {
        self.interfaces.remove(index)
    }

    fn iter(&self) -> core::slice::Iter<'_, H> {
        self.interfaces.iter()
    }

    fn iter_mut(&mut self) -> core::slice::IterMut<'_, H> {
        self.interfaces.iter_mut()
    }

    fn as_slice(&self) -> &[H] {
        &self.interfaces
    }
    fn as_mut_slice(&mut self) -> &mut [H] {
        &mut self.interfaces
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_past_any_fixed_cap_and_removes_in_place() {
        let mut set = GrowableInterfaceSet::<u16>::new();
        for n in 0..500u16 {
            assert_eq!(set.push(n), Ok(()));
        }
        assert_eq!(set.len(), 500);
        assert_eq!(set.remove(0), 0);
        assert_eq!(set.len(), 499);
        assert_eq!(*set.iter().next().unwrap(), 1);
    }

    #[test]
    fn as_slice_exposes_the_current_order() {
        let mut set = GrowableInterfaceSet::<u16>::new();
        assert_eq!(set.push(10), Ok(()));
        assert_eq!(set.push(20), Ok(()));
        assert_eq!(set.push(30), Ok(()));

        assert_eq!(set.as_slice(), &[10, 20, 30]);
    }
}
