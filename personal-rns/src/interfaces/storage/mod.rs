mod impls;

pub use impls::*;

pub trait InterfaceSet {
    type Item;

    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn push(&mut self, interface: Self::Item) -> Result<(), Self::Item>;

    fn remove(&mut self, index: usize) -> Self::Item;

    fn iter(&self) -> core::slice::Iter<'_, Self::Item>;
    fn iter_mut(&mut self) -> core::slice::IterMut<'_, Self::Item>;
    fn as_slice(&self) -> &[Self::Item];
    fn as_mut_slice(&mut self) -> &mut [Self::Item];
}
