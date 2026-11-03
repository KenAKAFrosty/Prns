use super::*;
use prns_groups_store::Mutation;

/// Catalog projections consume only authoritative facts staged in the same
/// native transaction. The key format/decoding remains owned by this module.
pub(in crate::enrollment) struct Summary {
    pub lifecycle: GroupLifecycle,
    pub ready: bool,
    pub member_count: u32,
    pub head: Option<[u8; 32]>,
}
pub(in crate::enrollment) enum Projection {
    Header(Summary),
    Added {
        invitation: [u8; 32],
        member: Member,
    },
    Request(Request),
    Terminal(Terminal),
}
pub(in crate::enrollment) fn visit_mutations(
    mutations: &[Mutation],
    mut visit: impl FnMut(Projection) -> Result<(), EnrollmentError>,
) -> Result<(), EnrollmentError> {
    for mutation in mutations {
        let Some(application) = prns_groups_mls::application_key(mutation.key()) else {
            continue;
        };
        if application == STATE_KEY {
            let bytes = mutation.value().ok_or(EnrollmentError::Corrupt)?;
            let header: Header =
                serde_json::from_slice(bytes).map_err(|_| EnrollmentError::Corrupt)?;
            if header.version != VERSION {
                return Err(EnrollmentError::UnsupportedVersion);
            }
            visit(Projection::Header(Summary {
                lifecycle: header.lifecycle,
                ready: header.ready()?,
                member_count: header.current.as_ref().map_or(Ok(0), |s| {
                    u32::try_from(s.manifest.members.len()).map_err(|_| EnrollmentError::Capacity)
                })?,
                head: header.current.as_ref().map(Signed::head).transpose()?,
            }))?;
        } else if let Some(suffix) = application.strip_prefix(PREFIX) {
            let Some((&kind, id)) = suffix.split_first() else {
                return Err(EnrollmentError::Corrupt);
            };
            match kind {
                ADDED => {
                    let row: Stored<Added> =
                        serde_json::from_slice(mutation.value().ok_or(EnrollmentError::Corrupt)?)
                            .map_err(|_| EnrollmentError::Corrupt)?;
                    let row = row.value;
                    if id != row.invitation {
                        return Err(EnrollmentError::Corrupt);
                    }
                    visit(Projection::Added {
                        invitation: row.invitation,
                        member: row.member,
                    })?;
                }
                TERMINAL => {
                    let row: Stored<Terminal> =
                        serde_json::from_slice(mutation.value().ok_or(EnrollmentError::Corrupt)?)
                            .map_err(|_| EnrollmentError::Corrupt)?;
                    if id != row.value.id()? {
                        return Err(EnrollmentError::Corrupt);
                    }
                    visit(Projection::Terminal(row.value))?;
                }
                REQUEST => {
                    let row: Stored<Request> =
                        serde_json::from_slice(mutation.value().ok_or(EnrollmentError::Corrupt)?)
                            .map_err(|_| EnrollmentError::Corrupt)?;
                    let row = row.value;
                    if id != row.id {
                        return Err(EnrollmentError::Corrupt);
                    }
                    visit(Projection::Request(row))?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
