//! Pure catalog/namespace inspection. Never calls initialize, maintenance,
//! pending-work generation, fixture cleanup or any recovery operation.
use super::*;
use crate::dependencies::{add, CatalogCursor};
use crate::{Dependencies, Error};
use prns_groups_service::{Descriptor, GroupService};
use prns_groups_store_redb::change::Stamp;

impl Enrollment<'_> {
    pub(crate) fn dependency_seed(&self, stamp: Stamp) -> Result<Dependencies, Error> {
        self.read()
            .map_err(Error::Enrollment)?
            .dependency_seed(stamp)
            .map_err(Error::Enrollment)
    }
}

impl<'db> Enrollment<'db> {
    pub(crate) fn dependency_page(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        cursor: &mut CatalogCursor,
        budget: usize,
        report: &mut Dependencies,
    ) -> Result<(), Error> {
        let read = self.read().map_err(Error::Enrollment)?;
        for _ in 0..budget {
            match read
                .dependency_step(cursor, report)
                .map_err(Error::Enrollment)?
            {
                indexed::dependencies::Step::Namespace { entry, archived } => {
                    self.owned_adapter(owner, &entry, None, |adapter, snapshot| {
                        Ok(inspect_namespace(
                            self, adapter, snapshot, &entry, archived, report,
                        ))
                    })
                    .map_err(Error::Enrollment)??;
                }
                indexed::dependencies::Step::CatalogRow => {}
                indexed::dependencies::Step::Complete => break,
            }
        }
        Ok(())
    }
}

fn inspect_namespace(
    enrollment: &Enrollment<'_>,
    adapter: Adapter<RedbBackend<'_>>,
    snapshot: Snapshot,
    entry: &Entry,
    archived: bool,
    report: &mut Dependencies,
) -> Result<(), Error> {
    let active = matches!(
        snapshot.lifecycle,
        GroupLifecycle::Active | GroupLifecycle::Leaving
    );
    if archived && (active || snapshot.current.is_none()) {
        return Err(Error::Corrupt);
    }
    if !archived && active {
        add(
            &mut report.active_memberships,
            u64::from(snapshot.current.is_some()),
        )?;
        add(
            &mut report.owner_groups,
            u64::from(
                snapshot.current.is_some() && entry.owner.rns == enrollment.local.into_bytes(),
            ),
        )?;
        if !snapshot.ready().map_err(Error::Enrollment)? {
            add(&mut report.incomplete_setup, 1)?;
        }
    }
    let Some(signed) = &snapshot.current else {
        // An initialized namespace without its first committed membership is
        // an unfinished create/join, not an empty, ready group.
        return Ok(());
    };
    let readers = signed
        .manifest
        .members
        .iter()
        .map(Member::endpoint)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::Enrollment)?;
    let head = prns_groups_core::ControlId(signed.head().map_err(Error::Enrollment)?);
    let service = GroupService::new_managed(
        adapter,
        Descriptor {
            group: GroupId(entry.group),
            control: prns_groups_core::ControlReference {
                head,
                era: prns_groups_core::AuthorizationEra(head),
            },
            readers: &readers,
        },
        entry.profile.service_limits().map_err(Error::Api)?,
    )?;
    let facts = service.dependencies()?;
    if facts.retired == active {
        return Err(Error::Corrupt);
    }
    add(
        &mut report.pending_publications,
        facts.pending_authored_records,
    )?;
    add(
        &mut report.pending_publication_targets,
        facts.pending_authored_targets,
    )?;
    add(&mut report.pending_receipts, facts.pending_receipts)?;
    add(&mut report.retained_records, facts.retained_records)?;
    add(
        &mut report.retained_result_changes,
        facts.retained_result_changes,
    )?;
    Ok(())
}
