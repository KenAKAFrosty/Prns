//! Exact fixed/managed view reads over the owner's selected provisioning.
//! Returned pages allocate only their selected slots; roster validation borrows
//! the existing provisioning and does not build a second complete roster.
use super::*;
use crate::{
    ConversationCapabilities, ConversationPage, ConversationSummary, MemberPage, PageLimits,
};
use std::mem::size_of;

pub(super) fn detail(
    cfg: &Provisioned,
    revision: u64,
    limit: PageLimits,
) -> Result<Conversation, Error> {
    let (_, bytes) = limit.checked().map_err(Error::Api)?;
    let fixed = size_of::<Conversation>();
    if cfg.title.is_empty() || cfg.title.len() > crate::profile::MAX_TITLE_BYTES {
        return Err(Error::Configuration);
    }
    if fixed.checked_add(cfg.title.len()).is_none_or(|n| n > bytes) {
        return Err(Error::InvalidInput);
    }
    validate_roster(cfg)?;
    let mut title = String::new();
    title
        .try_reserve_exact(cfg.title.len())
        .map_err(|_| Error::StorageFull)?;
    if fixed
        .checked_add(title.capacity())
        .is_none_or(|n| n > bytes)
    {
        return Err(Error::InvalidInput);
    }
    title.push_str(&cfg.title);
    let member_count = u64::try_from(cfg.readers.len()).map_err(|_| Error::Configuration)?;
    Ok(Conversation {
        owner_identity: cfg.owner_identity,
        lifecycle: cfg.lifecycle,
        capabilities: ConversationCapabilities::for_state(
            cfg.owner_identity,
            cfg.local_reticulum_identity,
            cfg.lifecycle,
            true,
            member_count,
        ),
        group: cfg.group,
        title,
        revision,
        control: cfg.control,
        member_count,
        // The managed catalog owns the authenticated profile and fills it after
        // selecting this exact group. Fixed provisioning has no managed profile.
        profile: None,
    })
}

pub(super) fn members(
    cfg: &Provisioned,
    after: Option<Endpoint>,
    limit: PageLimits,
) -> Result<MemberPage, Error> {
    let (rows, bytes) = limit.checked().map_err(Error::Api)?;
    let fixed = size_of::<MemberPage>();
    let row = size_of::<ConversationMember>();
    let slots = bytes.checked_sub(fixed).ok_or(Error::InvalidInput)? / row;
    if slots == 0 {
        return Err(Error::InvalidInput);
    }
    let local = validate_roster(cfg)?;
    let slots = rows.min(slots).min(cfg.readers.len());
    let mut members: Vec<ConversationMember> = Vec::new();
    members
        .try_reserve_exact(slots)
        .map_err(|_| Error::StorageFull)?;
    if members
        .capacity()
        .checked_mul(row)
        .and_then(|n| n.checked_add(fixed))
        .is_none_or(|n| n > bytes)
    {
        return Err(Error::InvalidInput);
    }
    let local_member = member(cfg, local, cfg.local_reticulum_identity, true);
    let mut eligible = 0usize;
    for candidate in core::iter::once(local_member).chain(
        cfg.peers
            .iter()
            .map(|peer| member(cfg, peer.endpoint, peer.identity, false)),
    ) {
        if after.is_some_and(|after| candidate.endpoint <= after) {
            continue;
        }
        eligible += 1;
        let index = match members.binary_search_by_key(&candidate.endpoint, |row| row.endpoint) {
            Ok(_) => return Err(Error::Configuration),
            Err(index) => index,
        };
        // Retain only the smallest bounded prefix. Both insertion and eviction
        // stay within the already charged return allocation.
        if index >= slots {
            continue;
        }
        if members.len() == slots {
            members.pop();
        }
        members.insert(index, candidate);
    }
    let next = if eligible > members.len() {
        Some(members.last().ok_or(Error::Corrupt)?.endpoint)
    } else {
        None
    };
    Ok(MemberPage {
        members,
        next,
        control: cfg.control,
    })
}

fn member(
    cfg: &Provisioned,
    endpoint: Endpoint,
    identity: prns_host::IdentityHash,
    is_local: bool,
) -> ConversationMember {
    ConversationMember {
        endpoint,
        identity,
        is_local,
        is_owner: cfg.owner_identity == Some(identity),
    }
}

/// Validate the whole selected roster, even when an invalid mapping would lie
/// beyond this page. This retains the fixed-provisioning contract without an
/// allocated Vec of all returned ConversationMember records.
pub(super) fn validate_roster(cfg: &Provisioned) -> Result<Endpoint, Error> {
    let binding = cfg.identity.binding();
    let local = Endpoint {
        id: prns_groups_core::EndpointId(binding.endpoint),
        incarnation: prns_groups_core::EndpointIncarnation::new(binding.incarnation)
            .map_err(|_| Error::Configuration)?,
    };
    if cfg.readers.is_empty()
        || cfg.readers.len() > cfg.mls_limits.max_members
        || cfg.readers.len() > cfg.service_limits.max_targets
        || cfg.peers.len().checked_add(1) != Some(cfg.readers.len())
        || !cfg.readers.contains(&local)
        || cfg.readers.iter().enumerate().any(|(index, reader)| {
            cfg.readers[..index]
                .iter()
                .any(|other| other.id == reader.id)
        })
        || cfg.peers.iter().enumerate().any(|(index, peer)| {
            peer.endpoint.id == local.id
                || peer.identity == cfg.local_reticulum_identity
                || !cfg.readers.contains(&peer.endpoint)
                || cfg.peers[..index]
                    .iter()
                    .any(|other| other.endpoint == peer.endpoint || other.identity == peer.identity)
        })
        || cfg.owner_identity.is_some_and(|owner| {
            owner != cfg.local_reticulum_identity
                && !cfg.peers.iter().any(|peer| peer.identity == owner)
        })
    {
        return Err(Error::Configuration);
    }
    Ok(local)
}

pub(super) fn summaries(
    cfg: &Provisioned,
    after: Option<prns_groups_core::GroupId>,
    limit: PageLimits,
) -> Result<ConversationPage, Error> {
    let (_, bytes) = limit.checked().map_err(Error::Api)?;
    validate_roster(cfg)?;
    if cfg.title.is_empty() || cfg.title.len() > crate::profile::MAX_TITLE_BYTES {
        return Err(Error::Configuration);
    }
    let fixed = size_of::<ConversationPage>();
    let row = size_of::<ConversationSummary>();
    if fixed
        .checked_add(row)
        .and_then(|n| n.checked_add(cfg.title.len()))
        .is_none_or(|n| n > bytes)
    {
        return Err(Error::InvalidInput);
    }
    if after.is_some_and(|after| cfg.group <= after) {
        return ConversationPage::single(None, limit).map_err(Error::Api);
    }
    let mut title = String::new();
    title
        .try_reserve_exact(cfg.title.len())
        .map_err(|_| Error::StorageFull)?;
    title.push_str(&cfg.title);
    let member_count = u64::try_from(cfg.readers.len()).map_err(|_| Error::Configuration)?;
    let summary = ConversationSummary {
        group: cfg.group,
        title,
        owner_identity: cfg.owner_identity,
        lifecycle: cfg.lifecycle,
        ready: true,
        member_count,
        control: Some(cfg.control),
        capabilities: ConversationCapabilities::for_state(
            cfg.owner_identity,
            cfg.local_reticulum_identity,
            cfg.lifecycle,
            true,
            member_count,
        ),
    };
    ConversationPage::single(Some(summary), limit).map_err(Error::Api)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use prns_groups_core::{EndpointId, EndpointIncarnation};
    use prns_host::IdentityHash;

    fn cfg() -> Provisioned {
        let identities = [IdentityHash::new([1; 16]), IdentityHash::new([2; 16])];
        let fixture =
            crate::fixture::generate("Members", &identities, Timestamp(1_792_000_000)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("members.redb")).unwrap();
        crate::fixture::load_and_install(&db, &fixture[0], identities[0]).unwrap()
    }

    fn selected_roster(count: u16) -> Provisioned {
        let mut cfg = cfg();
        let local = validate_roster(&cfg).unwrap();
        cfg.peers.clear();
        cfg.readers.clear();
        cfg.readers.push(local);
        cfg.mls_limits.max_members = usize::from(count);
        cfg.service_limits.max_targets = usize::from(count);
        for n in 1..count {
            let mut id = [0; 32];
            id[0] = local.id.0[0] ^ 255;
            id[1..3].copy_from_slice(&n.to_be_bytes());
            let endpoint = Endpoint {
                id: EndpointId(id),
                incarnation: EndpointIncarnation::new(u64::from(n)).unwrap(),
            };
            let mut identity = [0; 16];
            identity[0] = cfg.local_reticulum_identity.into_bytes()[0] ^ 255;
            identity[1..3].copy_from_slice(&n.to_be_bytes());
            cfg.peers.push(prns_groups_reticulum::Peer {
                endpoint,
                identity: IdentityHash::new(identity),
                destination: prns_groups_reticulum::destination_for(IdentityHash::new(identity))
                    .unwrap(),
            });
            cfg.readers.push(endpoint);
        }
        cfg.peers.reverse();
        cfg.readers.reverse();
        cfg
    }

    fn page_bytes(rows: usize) -> u64 {
        u64::try_from(size_of::<MemberPage>() + rows * size_of::<ConversationMember>()).unwrap()
    }

    #[test]
    fn member_pages_are_sorted_exclusive_complete_and_charge_actual_capacity() {
        // The selected native descriptor is the existing bounded roster scratch;
        // this helper never creates a second full ConversationMember collection.
        let cfg = selected_roster(256);
        let limit = PageLimits {
            max_rows: 7,
            max_bytes: page_bytes(3),
        };
        let mut after = None;
        let mut actual = Vec::new();
        loop {
            let page = members(&cfg, after, limit).unwrap();
            assert!(page.members.len() <= 3);
            assert!(page_bytes(page.members.capacity()) <= limit.max_bytes);
            assert_eq!(page.control, cfg.control);
            assert!(page
                .members
                .windows(2)
                .all(|pair| pair[0].endpoint < pair[1].endpoint));
            assert!(page
                .members
                .iter()
                .all(|m| after.is_none_or(|after| m.endpoint > after)));
            if page.next.is_some() {
                assert_eq!(page.next, page.members.last().map(|m| m.endpoint));
            }
            actual.extend(page.members.iter().map(|m| m.endpoint));
            after = page.next;
            if after.is_none() {
                break;
            }
        }
        let mut expected = cfg.readers.clone();
        expected.sort();
        assert_eq!(actual, expected);
        let empty = members(&cfg, expected.last().copied(), limit).unwrap();
        assert!(empty.members.is_empty());
        assert!(empty.next.is_none());
        let mut between = expected[0];
        between.incarnation = EndpointIncarnation::new(between.incarnation.get() + 1).unwrap();
        let page = members(&cfg, Some(between), limit).unwrap();
        assert_eq!(page.members[0].endpoint, expected[1]);
    }

    #[test]
    fn malformed_mappings_beyond_the_page_and_incompatible_budgets_fail() {
        let cfg = selected_roster(256);
        let limit = PageLimits {
            max_rows: 1,
            max_bytes: page_bytes(1),
        };
        let mut invalid = cfg.clone();
        invalid.peers[200].identity = invalid.peers[201].identity;
        assert!(matches!(
            members(&invalid, None, limit),
            Err(Error::Configuration)
        ));
        invalid = cfg.clone();
        invalid.peers[200].endpoint = invalid.peers[201].endpoint;
        assert!(matches!(
            members(&invalid, None, limit),
            Err(Error::Configuration)
        ));
        invalid = cfg.clone();
        invalid.readers[200] = invalid.readers[201];
        assert!(matches!(
            members(&invalid, None, limit),
            Err(Error::Configuration)
        ));
        for limit in [
            PageLimits {
                max_rows: 0,
                max_bytes: page_bytes(1),
            },
            PageLimits {
                max_rows: 1,
                max_bytes: page_bytes(1) - 1,
            },
            PageLimits {
                max_rows: 1,
                max_bytes: u64::MAX,
            },
        ] {
            assert!(members(&cfg, None, limit).is_err());
        }
    }

    #[test]
    fn exact_detail_and_fixed_summary_charge_title_and_expose_no_management() {
        let mut cfg = cfg();
        // Excess capacity on the source title must not be cloned into a result.
        cfg.title.reserve(4096);
        let limit = PageLimits {
            max_rows: 1,
            max_bytes: (size_of::<Conversation>() + cfg.title.len()) as u64,
        };
        let detail = detail(&cfg, 55, limit).unwrap();
        assert_eq!(detail.revision, 55);
        assert!(size_of::<Conversation>() + detail.title.capacity() <= limit.max_bytes as usize);
        assert_eq!(detail.member_count, 2);
        assert!(detail.profile.is_none());
        assert!(detail.capabilities.can_send);
        assert!(!detail.capabilities.can_invite);
        assert!(!detail.capabilities.can_remove_members);
        assert!(!detail.capabilities.can_leave);
        assert!(!detail.capabilities.can_close);
        assert!(!detail.capabilities.can_delete_local);
        assert!(super::detail(
            &cfg,
            55,
            PageLimits {
                max_bytes: limit.max_bytes - 1,
                ..limit
            }
        )
        .is_err());
        let summary_limit = PageLimits {
            max_rows: 1,
            max_bytes: (size_of::<ConversationPage>()
                + size_of::<ConversationSummary>()
                + cfg.title.len()) as u64,
        };
        let page = summaries(&cfg, None, summary_limit).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].capabilities, detail.capabilities);
        assert_eq!(page.items[0].control, Some(cfg.control));
        assert!(summaries(&cfg, Some(cfg.group), summary_limit)
            .unwrap()
            .items
            .is_empty());
        assert!(summaries(
            &cfg,
            None,
            PageLimits {
                max_bytes: summary_limit.max_bytes - 1,
                ..summary_limit
            }
        )
        .is_err());
        cfg.lifecycle = crate::GroupLifecycle::Closed;
        assert!(
            !super::detail(&cfg, 55, limit)
                .unwrap()
                .capabilities
                .can_send
        );
    }

    #[test]
    fn member_pages_report_control_changes_between_reads() {
        let mut cfg = cfg();
        let limit = PageLimits {
            max_rows: 1,
            max_bytes: page_bytes(1),
        };
        let first = members(&cfg, None, limit).unwrap();
        cfg.control.head = prns_groups_core::ControlId([99; 32]);
        cfg.control.era = prns_groups_core::AuthorizationEra(cfg.control.head);
        let second = members(&cfg, first.next, limit).unwrap();
        assert_ne!(first.control, second.control);
        assert_eq!(second.control, cfg.control);
    }
}
