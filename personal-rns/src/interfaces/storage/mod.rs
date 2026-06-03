//! Storage recipe for a node's registered interfaces — the runtime-owned set a
//! node speaks over.
//!
//! The element `H` is the host's choice. It could be one concrete interface handle, or the
//! host's own closed `enum` over the interface types it speaks. Heterogeneity is
//! the host's concern, not the collection's: this trait only knows how to hold,
//! add, remove, and walk a set of `H`.

mod impls;

pub use impls::*;

pub trait InterfaceSet {
    type Item;

    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append an interface. The fixed backing can be full, so a rejected interface
    /// is handed back in `Err`
    fn push(&mut self, interface: Self::Item) -> Result<(), Self::Item>;

    /// Remove and return the interface at `index`, shifting the rest down to keep
    /// order. `index` must be in bounds — the runtime gets it from [`iter`](Self::iter).
    fn remove(&mut self, index: usize) -> Self::Item;

    fn iter(&self) -> core::slice::Iter<'_, Self::Item>;
    fn iter_mut(&mut self) -> core::slice::IterMut<'_, Self::Item>;
}
