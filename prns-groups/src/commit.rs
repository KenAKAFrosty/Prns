//! Preserve optional standalone stamps and strict selected-owner commits.
use prns_groups_store::StoreIncarnation;
use prns_groups_store_redb::{change, ChangeWitness, RedbError};

pub(crate) fn commit(
    write: redb::WriteTransaction,
    witness: Option<&ChangeWitness>,
    incarnation: Option<StoreIncarnation>,
) -> Result<(), RedbError> {
    match incarnation {
        Some(incarnation) => change::commit_selected(write, witness, incarnation),
        None => change::commit(write, witness),
    }
}
