//! Author resolution belongs to the exact membership namespace. Read adapters
//! keep the same owning key/row decoder for committed and staged transactions.
use super::*;
use prns_groups_mls::ApplicationKey;
use prns_groups_store_redb::CommitRead;

pub(crate) trait Facts {
    fn application_value<T>(
        &self,
        key: &[u8],
        inspect: impl FnOnce(Option<&[u8]>) -> Result<T, EnrollmentError>,
    ) -> Result<T, EnrollmentError>;
}

impl Facts for Adapter<RedbBackend<'_>> {
    fn application_value<T>(
        &self,
        key: &[u8],
        inspect: impl FnOnce(Option<&[u8]>) -> Result<T, EnrollmentError>,
    ) -> Result<T, EnrollmentError> {
        let value = self
            .application_read(key)
            .map_err(|_| EnrollmentError::Corrupt)?;
        inspect(value.as_ref().map(|bytes| bytes.as_slice()))
    }
}

impl Facts for Coordinator<RedbBackend<'_>> {
    fn application_value<T>(
        &self,
        key: &[u8],
        inspect: impl FnOnce(Option<&[u8]>) -> Result<T, EnrollmentError>,
    ) -> Result<T, EnrollmentError> {
        let key = ApplicationKey::new(key).map_err(|_| EnrollmentError::Capacity)?;
        let value = self
            .read(key.bytes())
            .map_err(|_| EnrollmentError::Corrupt)?
            .map(Zeroizing::new);
        inspect(value.as_ref().map(|bytes| bytes.as_slice()))
    }
}

impl Facts for CommitRead<'_> {
    fn application_value<T>(
        &self,
        key: &[u8],
        inspect: impl FnOnce(Option<&[u8]>) -> Result<T, EnrollmentError>,
    ) -> Result<T, EnrollmentError> {
        let key = ApplicationKey::new(key).map_err(|_| EnrollmentError::Capacity)?;
        self.with_value(key.bytes(), inspect)
            .map_err(EnrollmentError::Storage)?
    }
}

pub(crate) fn historical_author(
    facts: &impl Facts,
    limits: RedbLimits,
    endpoint: Endpoint,
) -> Result<Option<IdentityHash>, EnrollmentError> {
    // Current roster lookup has already failed in the caller. Load only this
    // namespace's bounded header, one old binding and its reciprocal order key.
    // In a commit participant all three reads see the existing write snapshot.
    let snapshot = facts.application_value(STATE_KEY, |bytes| Snapshot::decode(bytes, limits))?;
    snapshot
        .past_member(facts, endpoint)
        .map(|member| member.map(|member| IdentityHash::new(member.rns)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::{cell::Cell, collections::BTreeMap};

    struct MemoryFacts {
        values: BTreeMap<Vec<u8>, Vec<u8>>,
        reads: Cell<usize>,
    }
    impl Facts for MemoryFacts {
        fn application_value<T>(
            &self,
            key: &[u8],
            inspect: impl FnOnce(Option<&[u8]>) -> Result<T, EnrollmentError>,
        ) -> Result<T, EnrollmentError> {
            self.reads.set(self.reads.get() + 1);
            inspect(self.values.get(key).map(Vec::as_slice))
        }
    }

    #[test]
    fn historical_authors_use_exact_incarnation_and_require_reciprocal_proof() {
        let member = Member::new(
            IdentityHash::new([51; 16]),
            LocalIdentity::generate([31; 32], 7).unwrap().binding(),
        );
        let endpoint = member.endpoint().unwrap();
        let header = Header {
            past_member_count: 1,
            ..Default::default()
        };
        let mut facts = MemoryFacts {
            values: BTreeMap::from([
                (STATE_KEY.to_vec(), encode(&header).unwrap()),
                (
                    key(PAST_MEMBER, &member_key(&member)),
                    encode(&Stored {
                        ordinal: 0,
                        value: PastMember {
                            member: member.clone(),
                            removed_epoch: 5,
                        },
                    })
                    .unwrap(),
                ),
                (
                    key(PAST_MEMBER_ORDER, &0u64.to_be_bytes()),
                    encode(&member_key(&member).to_vec()).unwrap(),
                ),
            ]),
            reads: Cell::new(0),
        };
        let limits = profile::store_limits();
        assert_eq!(
            historical_author(&facts, limits, endpoint).unwrap(),
            Some(IdentityHash::new(member.rns))
        );
        assert_eq!(
            facts.reads.get(),
            3,
            "header plus exact row and reciprocal key"
        );
        let other_incarnation = Endpoint {
            incarnation: EndpointIncarnation::new(8).unwrap(),
            ..endpoint
        };
        assert_eq!(
            historical_author(&facts, limits, other_incarnation).unwrap(),
            None
        );
        facts.values.insert(
            key(PAST_MEMBER_ORDER, &0u64.to_be_bytes()),
            encode(&[99u8; 40].to_vec()).unwrap(),
        );
        assert!(matches!(
            historical_author(&facts, limits, endpoint),
            Err(EnrollmentError::Corrupt)
        ));
        facts.values.insert(
            key(PAST_MEMBER_ORDER, &0u64.to_be_bytes()),
            encode(&member_key(&member).to_vec()).unwrap(),
        );
        facts
            .values
            .insert(STATE_KEY.to_vec(), encode(&Header::default()).unwrap());
        assert!(matches!(
            historical_author(&facts, limits, endpoint),
            Err(EnrollmentError::Corrupt)
        ));
    }
}
