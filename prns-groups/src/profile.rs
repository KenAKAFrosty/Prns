//! Managed groups' immutable interoperability limits and local resource policy.
//! A supported member ceiling does not reserve storage for unused memberships.
//! Byte limits remain independent quotas, not a promise that all can be filled
//! simultaneously. A namespace's selected limits cannot change on reopen.
use prns_groups_core::Seconds;
use prns_groups_store_redb::RedbLimits;

/// UTF-8 bytes in a native group title, before JSON escaping on enrollment wire.
pub const MAX_TITLE_BYTES: usize = 128;

/// Limits authenticated by managed enrollment and retained in creation input.
///
/// Peers must support the whole selected profile before joining. Local catalog,
/// archive, queue and working-set budgets are separate; this record does not
/// allocate or reserve `max_members` memberships. All byte counts describe the
/// encoded representations named by the field, not physical database usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupProfile"))]
pub struct GroupProfile {
    pub max_members: u32,
    pub max_record_body_bytes: u32,
    pub max_plaintext_bytes: u32,
    pub max_wire_bytes: u32,
    pub max_snapshot_bytes: u32,
    pub max_enrollment_frame_bytes: u32,
}

impl Default for GroupProfile {
    fn default() -> Self {
        Self::DEFAULT
    }
}

mod support;
pub use support::ProfileSupport;

/// Resolve the library's small default policy before retaining a Create input.
/// An embedding can select another checked profile; this is not a native ceiling.
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn default_group_profile() -> GroupProfile {
    GroupProfile::default()
}

/// Resolve the encoded record and enrollment bounds for an explicit member
/// ceiling. Wire and snapshot storage remain the embedding's byte policies;
/// validating a profile does not promise those budgets will admit every roster
/// or reserve space for unused members. Actual admission remains authoritative.
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn resolve_group_profile(
    max_members: u32,
    max_record_body_bytes: u32,
    max_wire_bytes: u32,
    max_snapshot_bytes: u32,
) -> Result<GroupProfile, crate::ApiError> {
    let invalid = || crate::ApiError::InvalidInput;
    let plaintext = prns_groups_service::encoded_record_len(
        host_size(max_members)?,
        host_size(max_record_body_bytes)?,
    )
    .map_err(|_| invalid())?;
    let mut profile = GroupProfile {
        max_members,
        max_record_body_bytes,
        max_plaintext_bytes: u32::try_from(plaintext).map_err(|_| invalid())?,
        max_wire_bytes,
        max_snapshot_bytes,
        // This field appears inside the authenticated frame. Its widest u32
        // encoding bounds its own decimal width without iterative allocation.
        max_enrollment_frame_bytes: u32::MAX,
    };
    profile.max_enrollment_frame_bytes =
        u32::try_from(crate::enrollment::required_frame_bytes(profile).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    profile.validate()?;
    Ok(profile)
}

impl GroupProfile {
    /// Library default policy, resolved into each immutable creation request.
    pub const DEFAULT: Self = Self {
        max_members: 8,
        max_record_body_bytes: 2054,
        max_plaintext_bytes: 16_384,
        max_wire_bytes: 65_536,
        max_snapshot_bytes: 262_144,
        max_enrollment_frame_bytes: 131_072,
    };

    /// Check representation and codec relationships without allocating records,
    /// loading a group or reserving storage. Local admission checks are separate.
    pub fn validate(self) -> Result<(), crate::ApiError> {
        let mls = self.mls_limits()?;
        let body = host_size(self.max_record_body_bytes)?;
        let frame = host_size(self.max_enrollment_frame_bytes)?;
        if prns_groups_service::encoded_record_len(mls.max_members, body)
            .map_err(|_| crate::ApiError::InvalidInput)?
            > mls.max_plaintext_bytes
            || frame
                < crate::enrollment::required_frame_bytes(self)
                    .map_err(|_| crate::ApiError::InvalidInput)?
        {
            return Err(crate::ApiError::InvalidInput);
        }
        Ok(())
    }

    /// The MLS adapter still owns validation of its limits. The remaining
    /// fields below are local ratchet/output policy, independent of membership.
    pub fn mls_limits(self) -> Result<prns_groups_mls::Limits, crate::ApiError> {
        let limits = prns_groups_mls::Limits {
            max_members: host_size(self.max_members)?,
            max_wire_bytes: host_size(self.max_wire_bytes)?,
            max_plaintext_bytes: host_size(self.max_plaintext_bytes)?,
            max_snapshot_bytes: host_size(self.max_snapshot_bytes)?,
            ..mls_limits()
        };
        limits
            .validate()
            .map_err(|_| crate::ApiError::InvalidInput)?;
        Ok(limits)
    }

    /// Existing service retention policy with this group's authenticated
    /// content and target bounds. A caller must still fund local obligations.
    pub fn service_limits(self) -> Result<prns_groups_service::Limits, crate::ApiError> {
        self.validate()?;
        Ok(prns_groups_service::Limits {
            max_targets: host_size(self.max_members)?,
            max_body_bytes: host_size(self.max_record_body_bytes)?,
            ..service_limits()
        })
    }
}

fn host_size(value: u32) -> Result<usize, crate::ApiError> {
    usize::try_from(value).map_err(|_| crate::ApiError::InvalidInput)
}

pub const MAX_MEMBERS: usize = GroupProfile::DEFAULT.max_members as usize;
/// Existing native record envelope ceiling; independent of a selected content codec.
pub const MAX_RECORD_BODY_BYTES: usize = GroupProfile::DEFAULT.max_record_body_bytes as usize;
pub const RETRY_POLICY: prns_groups_core::retry::RetryPolicy =
    prns_groups_core::retry::RetryPolicy {
        horizon_secs: 14 * 24 * 60 * 60,
        future_skew_secs: 300,
    };
pub fn store_limits() -> RedbLimits {
    RedbLimits {
        max_changes: 128,
        max_key_bytes: 256,
        max_value_bytes: 2_097_152,
        max_staged_bytes: 4_194_304,
        max_live_objects: 8192,
        max_live_bytes: 16_777_216,
        max_receipts: 4096,
    }
}
pub fn mls_limits() -> prns_groups_mls::Limits {
    prns_groups_mls::Limits {
        max_members: MAX_MEMBERS,
        max_groups: 1,
        max_wire_bytes: GroupProfile::DEFAULT.max_wire_bytes as usize,
        max_plaintext_bytes: GroupProfile::DEFAULT.max_plaintext_bytes as usize,
        max_snapshot_bytes: GroupProfile::DEFAULT.max_snapshot_bytes as usize,
        max_key_packages: 16,
        max_retained_epochs: 1,
        max_pending_proposals: 16,
        max_pending_outputs: 1024,
    }
}
pub fn service_limits() -> prns_groups_service::Limits {
    prns_groups_service::Limits {
        max_records: 256,
        max_targets: MAX_MEMBERS,
        max_body_bytes: MAX_RECORD_BODY_BYTES,
        max_changes: 4096,
        max_envelopes: 4096,
        max_retained_wire_bytes: 8_388_608,
        maximum_window: Seconds(14 * 24 * 60 * 60),
        future_skew: Seconds(300),
    }
}

/// Ordinary traffic leaves enough native space for every current non-owner's
/// removal and a terminal close. This is independent of the snapshot byte fence.
pub fn management_reserve(
    native: RedbLimits,
    mls: prns_groups_mls::Limits,
    members: usize,
) -> Result<prns_groups_store_redb::RedbReserve, prns_groups_store_redb::RedbError> {
    reserve_with_control_bytes(native, mls, members, mls.max_wire_bytes)
}

/// Managed enrollment additionally retains a signed control roster. Size that
/// exact owning representation at the admitted roster, never the unused ceiling.
pub(crate) fn managed_cleanup_reserve(
    native: RedbLimits,
    profile: GroupProfile,
    members: usize,
) -> Result<prns_groups_store_redb::RedbReserve, prns_groups_store_redb::RedbError> {
    let invalid = || prns_groups_store_redb::RedbError::Configuration;
    let mls = profile.mls_limits().map_err(|_| invalid())?;
    let control = crate::enrollment::required_frame_bytes_for_members(profile, members)
        .map_err(|_| invalid())?;
    let output = prns_groups_mls::Output::control_retention_bytes(mls.max_wire_bytes)
        .map_err(|_| invalid())?;
    let retained = control.checked_add(output).ok_or_else(invalid)?;
    reserve_with_control_bytes(native, mls, members, retained)
}
// Covers maximum-width terminal/request/past-member/ack payloads, reciprocal
// index values, and all keys for the sixteen cleanup slots below. The keyed
// state regression serializes those actual representations against this margin.
pub(crate) const CLEANUP_FACT_OVERHEAD: usize = 8192 + 512;

fn reserve_with_control_bytes(
    native: RedbLimits,
    mls: prns_groups_mls::Limits,
    members: usize,
    control_bytes: usize,
) -> Result<prns_groups_store_redb::RedbReserve, prns_groups_store_redb::RedbError> {
    let overflow = || prns_groups_store_redb::RedbError::Configuration;
    let max_value_bytes = native.transaction_limits()?.max_value_bytes;
    Ok(prns_groups_store_redb::RedbReserve {
        // Retain the existing housekeeping margin; additional actual members
        // fund their own removal/output rows rather than the supported ceiling.
        objects: 128.max(members.checked_mul(16).ok_or_else(overflow)?),
        bytes: control_bytes
            .checked_add(CLEANUP_FACT_OVERHEAD)
            .and_then(|per_member| per_member.checked_mul(members))
            .and_then(|members| members.checked_add(mls.max_snapshot_bytes))
            .and_then(|bytes| bytes.checked_add(max_value_bytes))
            .ok_or_else(overflow)?,
        // Each control may also retire the preceding output in a separate commit.
        receipts: members
            .checked_mul(2)
            .and_then(|n| n.checked_add(2))
            .ok_or_else(overflow)?,
    })
}
pub fn management_output_reserve(
    members: usize,
) -> Result<usize, prns_groups_store_redb::RedbError> {
    members
        .checked_add(2)
        .ok_or(prns_groups_store_redb::RedbError::Configuration)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn cleanup_reservation_follows_actual_members_not_the_supported_ceiling() {
        let small = selected(8).mls_limits().unwrap();
        let large = selected(256).mls_limits().unwrap();
        let before = management_reserve(store_limits(), small, 2).unwrap();
        let supported = management_reserve(store_limits(), large, 2).unwrap();
        assert_eq!(
            (before.objects, before.bytes, before.receipts),
            (supported.objects, supported.bytes, supported.receipts)
        );
        let admitted = management_reserve(store_limits(), large, 3).unwrap();
        assert!(admitted.bytes > supported.bytes);
        assert!(admitted.receipts > supported.receipts);
        assert!(management_reserve(store_limits(), large, usize::MAX).is_err());
    }

    fn selected(members: u32) -> GroupProfile {
        GroupProfile {
            max_members: members,
            max_record_body_bytes: 2054,
            max_plaintext_bytes: 16_384,
            max_wire_bytes: 65_536,
            max_snapshot_bytes: 262_144,
            max_enrollment_frame_bytes: 524_288,
        }
    }

    #[test]
    fn selected_capacity_is_checked_by_encoded_size_not_a_native_member_constant() {
        for members in [2, 8, 256] {
            let profile = selected(members);
            profile.validate().unwrap();
            assert_eq!(profile.mls_limits().unwrap().max_members, members as usize);
            assert_eq!(
                profile.service_limits().unwrap().max_targets,
                members as usize
            );
        }
        let mut too_large = selected(512);
        assert_eq!(too_large.validate(), Err(crate::ApiError::InvalidInput));
        too_large.max_plaintext_bytes = 32_768;
        too_large.validate().unwrap();
        assert_eq!(selected(0).validate(), Err(crate::ApiError::InvalidInput));
        assert_eq!(
            selected(u32::MAX).validate(),
            Err(crate::ApiError::InvalidInput)
        );
    }

    #[test]
    fn profile_rejects_inconsistent_byte_limits_and_unknown_fields() {
        let mut profile = selected(256);
        profile.max_plaintext_bytes = profile.max_wire_bytes + 1;
        assert_eq!(profile.validate(), Err(crate::ApiError::InvalidInput));
        profile = selected(256);
        // Raw MLS bytes fitting does not mean their base64 field and the two
        // authenticated rosters fit the actual enrollment frame.
        profile.max_enrollment_frame_bytes = profile.max_wire_bytes;
        assert_eq!(profile.validate(), Err(crate::ApiError::InvalidInput));
        // The selected byte budget is itself authenticated in both manifests;
        // settle its decimal width before checking the exact encoded boundary.
        for _ in 0..10 {
            profile.max_enrollment_frame_bytes =
                u32::try_from(crate::enrollment::required_frame_bytes(profile).unwrap()).unwrap();
        }
        profile.validate().unwrap();
        profile.max_enrollment_frame_bytes -= 1;
        assert_eq!(profile.validate(), Err(crate::ApiError::InvalidInput));
        profile = selected(256);
        profile.max_snapshot_bytes = 0;
        assert_eq!(profile.validate(), Err(crate::ApiError::InvalidInput));
        let encoded = serde_json::to_vec(&selected(256)).unwrap();
        assert_eq!(
            serde_json::from_slice::<GroupProfile>(&encoded).unwrap(),
            selected(256)
        );
        let mut extra = serde_json::to_value(selected(256)).unwrap();
        extra["unreviewed_limit"] = 1.into();
        assert!(serde_json::from_value::<GroupProfile>(extra).is_err());
    }

    #[test]
    fn resolved_default_is_valid_and_retains_the_existing_small_policy() {
        let profile = default_group_profile();
        profile.validate().unwrap();
        assert_eq!(profile.mls_limits().unwrap(), mls_limits());
        assert_eq!(profile.service_limits().unwrap(), service_limits());
    }

    #[test]
    fn resolved_capacity_uses_owning_encoded_bounds_without_a_fixed_member_cap() {
        for members in [2, 8, 256, 512] {
            let profile = resolve_group_profile(members, 2054, 65_536, 262_144).unwrap();
            profile.validate().unwrap();
            assert_eq!(profile.max_members, members);
            assert_eq!(
                profile.max_plaintext_bytes as usize,
                prns_groups_service::encoded_record_len(members as usize, 2054).unwrap()
            );
            assert!(
                profile.max_enrollment_frame_bytes as usize
                    >= crate::enrollment::required_frame_bytes(profile).unwrap()
            );
        }
        for (members, body, wire, snapshot) in [
            (0, 2054, 65_536, 262_144),
            (u32::MAX, 2054, 65_536, 262_144),
            (256, u32::MAX, 65_536, 262_144),
            (256, 2054, 128, 262_144),
            (256, 2054, 65_536, 0),
        ] {
            assert_eq!(
                resolve_group_profile(members, body, wire, snapshot),
                Err(crate::ApiError::InvalidInput)
            );
        }
    }
}
