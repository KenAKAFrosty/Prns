use heapless::Vec;

use crate::interfaces::storage::InterfaceSet;
use crate::interfaces::MAX_REGISTERED_INTERFACES;

#[derive(Debug)]
pub struct FixedInterfaceSet<H, const CAPACITY: usize = MAX_REGISTERED_INTERFACES> {
    interfaces: Vec<H, CAPACITY>,
}

impl<H, const CAPACITY: usize> FixedInterfaceSet<H, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            interfaces: Vec::new(),
        }
    }
}

// Manual (not derived) so the empty default never demands `H: Default` — an
// interface handle has no meaningful default, and recipes construct the set empty.
impl<H, const CAPACITY: usize> Default for FixedInterfaceSet<H, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H, const CAPACITY: usize> InterfaceSet for FixedInterfaceSet<H, CAPACITY> {
    type Item = H;

    fn len(&self) -> usize {
        self.interfaces.len()
    }

    fn push(&mut self, interface: H) -> Result<(), H> {
        self.interfaces.push(interface)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_to_capacity_then_hands_the_overflow_back() {
        let mut set = FixedInterfaceSet::<u16, 2>::new();
        assert!(set.is_empty());
        assert_eq!(set.push(10), Ok(()));
        assert_eq!(set.push(20), Ok(()));
        assert_eq!(set.len(), 2);
        assert_eq!(set.push(30), Err(30));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn remove_returns_the_interface_and_keeps_the_rest_in_order() {
        let mut set = FixedInterfaceSet::<u16, 4>::new();
        let _ = set.push(10);
        let _ = set.push(20);
        let _ = set.push(30);
        assert_eq!(set.remove(1), 20);
        let remaining: std::vec::Vec<u16> = set.iter().copied().collect();
        assert_eq!(remaining, std::vec![10, 30]);
    }
}
