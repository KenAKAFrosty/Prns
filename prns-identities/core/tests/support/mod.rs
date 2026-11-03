use prns_identities_core::{Catalog, Storage};

/// Failures belong to the durable adapter, independently of the coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    BeforeWrite,
    AfterWrite,
    StaleRevision,
    Read,
}

#[derive(Clone)]
pub struct Durable<const I: usize, const O: usize, const S: usize> {
    pub catalog: Catalog<I, O, S>,
    pub revision: u64,
    pub writes: usize,
    pub fail_write: Option<(usize, Failure)>,
    pub fail_read: bool,
}

impl<const I: usize, const O: usize, const S: usize> Durable<I, O, S> {
    pub fn new(catalog: Catalog<I, O, S>) -> Self {
        let revision = catalog.revision();
        Self {
            catalog,
            revision,
            writes: 0,
            fail_write: None,
            fail_read: false,
        }
    }

    pub fn fail_next(&mut self, failure: Failure) {
        self.fail_write = Some((self.writes + 1, failure));
    }

    /// A reboot retains only durable bytes, never an injected transient failure.
    pub fn reboot(&mut self) {
        self.fail_write = None;
        self.fail_read = false;
    }
}

impl<const I: usize, const O: usize, const S: usize> Storage<I, O, S> for Durable<I, O, S> {
    type Error = Failure;

    fn load(&mut self, into: &mut Catalog<I, O, S>) -> Result<(), Self::Error> {
        if self.fail_read {
            return Err(Failure::Read);
        }
        into.copy_from(&self.catalog);
        Ok(())
    }

    fn compare_and_store(
        &mut self,
        expected_revision: u64,
        next: &Catalog<I, O, S>,
    ) -> Result<(), Self::Error> {
        self.writes += 1;
        let failure = self
            .fail_write
            .filter(|(write, _)| *write == self.writes)
            .map(|(_, failure)| failure);
        if failure == Some(Failure::BeforeWrite) {
            return Err(Failure::BeforeWrite);
        }
        if expected_revision != self.revision {
            return Err(Failure::StaleRevision);
        }
        self.catalog.copy_from(next);
        self.revision = next.revision();
        if failure == Some(Failure::AfterWrite) {
            return Err(Failure::AfterWrite);
        }
        Ok(())
    }
}
