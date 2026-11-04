//! Shared representation for independently selected native query budgets.
use crate::ApiError;

/// Local bounds on one returned page, independent of total stored rows.
/// Each owning query accounts for its collection slots and dynamic value
/// capacities. Encoded-row/decoder scratch limits and allocator overhead are
/// separate; this record is not a process memory or storage guarantee.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupPageLimits"))]
pub struct PageLimits {
    pub max_rows: u32,
    pub max_bytes: u64,
}

impl PageLimits {
    /// Owners additionally check that a valid maximum row can fit a page, so
    /// truncation cannot return an empty page while eligible rows remain.
    pub fn validate(self) -> Result<(), ApiError> {
        self.checked().map(|_| ())
    }

    pub(crate) fn checked(self) -> Result<(usize, usize), ApiError> {
        let rows = usize::try_from(self.max_rows).map_err(|_| ApiError::InvalidInput)?;
        let bytes = usize::try_from(self.max_bytes).map_err(|_| ApiError::InvalidInput)?;
        if rows == 0 || bytes == 0 || bytes > isize::MAX as usize {
            return Err(ApiError::InvalidInput);
        }
        Ok((rows, bytes))
    }
}

impl crate::ConversationPage {
    /// The same owning return-allocation policy for a fixed catalog or a
    /// group-scoped exact summary. This never loads other catalog rows.
    pub(crate) fn single(
        item: Option<crate::ConversationSummary>,
        limit: PageLimits,
    ) -> Result<Self, ApiError> {
        let (_, bytes) = limit.checked()?;
        let fixed = std::mem::size_of::<Self>();
        if fixed > bytes {
            return Err(ApiError::InvalidInput);
        }
        let mut items = Vec::new();
        if let Some(item) = item {
            let row = std::mem::size_of::<crate::ConversationSummary>();
            let required = fixed
                .checked_add(row)
                .and_then(|n| n.checked_add(item.title.capacity()))
                .ok_or(ApiError::InvalidInput)?;
            if required > bytes {
                return Err(ApiError::InvalidInput);
            }
            items
                .try_reserve_exact(1)
                .map_err(|_| ApiError::StorageFull)?;
            if items
                .capacity()
                .checked_mul(row)
                .and_then(|n| n.checked_add(fixed))
                .and_then(|n| n.checked_add(item.title.capacity()))
                .is_none_or(|n| n > bytes)
            {
                return Err(ApiError::InvalidInput);
            }
            items.push(item);
        }
        Ok(Self { items, next: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_page_allocations_fail_before_query_work() {
        for limits in [
            PageLimits {
                max_rows: 0,
                max_bytes: 1,
            },
            PageLimits {
                max_rows: 1,
                max_bytes: 0,
            },
            PageLimits {
                max_rows: 1,
                max_bytes: u64::MAX,
            },
        ] {
            assert_eq!(limits.validate(), Err(ApiError::InvalidInput));
        }
        assert_eq!(
            PageLimits {
                max_rows: 1_024,
                max_bytes: 1_048_576
            }
            .validate(),
            Ok(())
        );
    }

    #[test]
    fn single_summary_charges_owned_capacity_and_empty_page_header() {
        use crate::{
            ConversationCapabilities, ConversationPage, ConversationSummary, GroupLifecycle,
        };
        let summary = || {
            let mut title = String::with_capacity(256);
            title.push('g');
            ConversationSummary {
                group: prns_groups_core::GroupId([1; 32]),
                title,
                owner_identity: None,
                lifecycle: GroupLifecycle::Active,
                ready: true,
                member_count: 2,
                control: None,
                capabilities: ConversationCapabilities::for_state(
                    None,
                    prns_host::IdentityHash::new([2; 16]),
                    GroupLifecycle::Active,
                    true,
                    2,
                ),
            }
        };
        let fixed = std::mem::size_of::<ConversationPage>();
        let row = std::mem::size_of::<ConversationSummary>();
        assert_eq!(
            ConversationPage::single(
                Some(summary()),
                PageLimits {
                    max_rows: 1,
                    max_bytes: (fixed + row + 1) as u64,
                }
            ),
            Err(ApiError::InvalidInput),
        );
        assert!(ConversationPage::single(
            Some(summary()),
            PageLimits {
                max_rows: 1,
                max_bytes: 4096,
            }
        )
        .is_ok());
        assert_eq!(
            ConversationPage::single(
                None,
                PageLimits {
                    max_rows: 1,
                    max_bytes: (fixed - 1) as u64,
                }
            ),
            Err(ApiError::InvalidInput),
        );
        assert_eq!(
            ConversationPage::single(
                None,
                PageLimits {
                    max_rows: 1,
                    max_bytes: fixed as u64,
                }
            ),
            Ok(ConversationPage {
                items: Vec::new(),
                next: None
            }),
        );
    }

    #[test]
    fn capabilities_preserve_authority_and_do_not_offer_fixed_group_management() {
        use crate::{ConversationCapabilities as Capabilities, GroupLifecycle as Lifecycle};
        let local = prns_host::IdentityHash::new([1; 16]);
        let owner = prns_host::IdentityHash::new([2; 16]);
        let fixed = Capabilities::for_state(None, local, Lifecycle::Active, true, 2);
        assert!(fixed.can_send);
        assert!(
            !(fixed.can_delete_local || fixed.can_invite || fixed.can_close || fixed.can_leave)
        );
        assert!(!fixed.can_remove_members);
        let waiting = Capabilities::for_state(Some(local), local, Lifecycle::Active, false, 2);
        assert!(!(waiting.can_send || waiting.can_close || waiting.can_invite));
        let sole_owner = Capabilities::for_state(Some(local), local, Lifecycle::Active, true, 1);
        assert!(sole_owner.can_close && sole_owner.can_invite);
        assert!(
            !(sole_owner.can_send || sole_owner.can_remove_members || sole_owner.can_delete_local)
        );
        let removed = Capabilities::for_state(Some(owner), local, Lifecycle::Removed, true, 2);
        assert!(removed.can_delete_local);
        assert!(!(removed.can_send || removed.can_leave || removed.can_remove_members));
        let closed = Capabilities::for_state(Some(local), local, Lifecycle::Closed, true, 1);
        assert!(closed.can_delete_local);
        assert!(!closed.can_close);
    }
}
