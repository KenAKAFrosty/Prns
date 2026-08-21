//! The sender answering a part request: RNS 1.4.2 `Resource.request`.
//!
//! Requested names are picked out of the serving scope (the receiver's minimum consecutive height through the collision guard span) in ascending part order.
//! A hashmap-exhausted request additionally works out the next segment of names and slides the scope forward.
//! Plain part requests never move the scope.

use crate::routing::links::resources::control::PART_REQUEST_PLAINTEXT_CAP;
use crate::routing::links::resources::{
    map_hash_name_word, COLLISION_GUARD_SIZE, HASHMAP_MAX_LEN, MAP_HASH_LEN, RESOURCE_HASH_LEN,
    WINDOW_MAX,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashmapUpdatePlanError {
    LastMapHashNotInScope,
    NotOnSegmentBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashmapUpdatePlan {
    pub segment: u64,
    pub entries_start: usize,
    pub entries_end: usize,
    pub scope_start: usize,
}

/// The most names a part request can fit in its plaintext after the flag and resource hash.
/// Normal emitters cap at `WINDOW_MAX`; the extra slot preserves the old read side's tolerance for a full base-MDU request without a hashmap-exhausted marker.
pub const MAX_REQUESTED_PARTS: usize =
    (PART_REQUEST_PLAINTEXT_CAP - 1 - RESOURCE_HASH_LEN) / MAP_HASH_LEN;

#[derive(Debug, Clone)]
pub struct ServedPartIndices {
    parts: [usize; MAX_REQUESTED_PARTS],
    len: usize,
    next: usize,
}

impl ServedPartIndices {
    fn empty() -> Self {
        Self {
            parts: [0; MAX_REQUESTED_PARTS],
            len: 0,
            next: 0,
        }
    }

    fn push(&mut self, part: usize) -> bool {
        if self.len == self.parts.len() {
            return false;
        }
        self.parts[self.len] = part;
        self.len += 1;
        true
    }
}

impl Iterator for ServedPartIndices {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.len {
            return None;
        }
        let part = self.parts[self.next];
        self.next += 1;
        Some(part)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.len.saturating_sub(self.next);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for ServedPartIndices {}

fn serving_scope(hashmap: &[u8], scope_start: usize) -> core::ops::Range<usize> {
    let known = hashmap.len() / MAP_HASH_LEN;
    scope_start..scope_start.saturating_add(COLLISION_GUARD_SIZE).min(known)
}

pub fn serve_part_indices(
    hashmap: &[u8],
    scope_start: usize,
    requested: &[u8],
) -> ServedPartIndices {
    let mut served = ServedPartIndices::empty();
    let (requested_names, _) = requested.as_chunks::<MAP_HASH_LEN>();
    let requested_len = requested_names.len();

    if requested_len <= MAX_REQUESTED_PARTS {
        let mut requested_name_words = [0u32; MAX_REQUESTED_PARTS];
        for (index, asked) in requested_names.iter().enumerate() {
            requested_name_words[index] = map_hash_name_word(asked);
        }
        for i in serving_scope(hashmap, scope_start) {
            let name = &hashmap[i * MAP_HASH_LEN..(i + 1) * MAP_HASH_LEN];
            if requested_name_words[..requested_len].contains(&map_hash_name_word(name))
                && !served.push(i)
            {
                break;
            }
        }
        return served;
    }

    for i in serving_scope(hashmap, scope_start) {
        let name = &hashmap[i * MAP_HASH_LEN..(i + 1) * MAP_HASH_LEN];
        if requested_names.iter().any(|asked| asked == name) && !served.push(i) {
            break;
        }
    }
    served
}

pub fn plan_hashmap_update(
    hashmap: &[u8],
    scope_start: usize,
    last_known: &[u8; MAP_HASH_LEN],
) -> Result<HashmapUpdatePlan, HashmapUpdatePlanError> {
    let known = hashmap.len() / MAP_HASH_LEN;
    let matched = serving_scope(hashmap, scope_start)
        .find(|&i| hashmap[i * MAP_HASH_LEN..(i + 1) * MAP_HASH_LEN] == *last_known)
        .ok_or(HashmapUpdatePlanError::LastMapHashNotInScope)?;
    let past_matched = matched + 1;
    if !past_matched.is_multiple_of(HASHMAP_MAX_LEN) {
        return Err(HashmapUpdatePlanError::NotOnSegmentBoundary);
    }
    let segment = past_matched / HASHMAP_MAX_LEN;
    Ok(HashmapUpdatePlan {
        segment: segment as u64,
        entries_start: segment * HASHMAP_MAX_LEN,
        entries_end: ((segment + 1) * HASHMAP_MAX_LEN).min(known),
        scope_start: past_matched.saturating_sub(1 + WINDOW_MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(count: usize) -> std::vec::Vec<u8> {
        let mut hashmap = std::vec::Vec::new();
        for i in 0..count as u32 {
            hashmap.extend_from_slice(&i.to_be_bytes());
        }
        hashmap
    }

    fn name(i: u32) -> [u8; MAP_HASH_LEN] {
        i.to_be_bytes()
    }

    fn collect(iter: impl Iterator<Item = usize>) -> std::vec::Vec<usize> {
        iter.collect()
    }

    #[test]
    fn requested_parts_come_back_in_part_order_not_request_order() {
        let hashmap = names(300);
        let mut requested = std::vec::Vec::new();
        requested.extend_from_slice(&name(10));
        requested.extend_from_slice(&name(3));
        requested.extend_from_slice(&name(112));
        assert_eq!(
            collect(serve_part_indices(&hashmap, 0, &requested)),
            std::vec![3, 10, 112],
        );
    }

    #[test]
    fn names_beyond_the_collision_guard_or_unknown_are_ignored() {
        let hashmap = names(300);
        let mut requested = std::vec::Vec::new();
        requested.extend_from_slice(&name(250));
        requested.extend_from_slice(&name(5));
        requested.extend_from_slice(&name(9_999));
        assert_eq!(
            collect(serve_part_indices(&hashmap, 0, &requested)),
            std::vec![5],
        );
        assert_eq!(
            collect(serve_part_indices(&hashmap, 100, &requested)),
            std::vec![250],
        );
    }

    #[test]
    fn the_scope_floor_excludes_parts_behind_it() {
        let hashmap = names(300);
        let mut requested = std::vec::Vec::new();
        requested.extend_from_slice(&name(50));
        assert_eq!(
            collect(serve_part_indices(&hashmap, 100, &requested)),
            std::vec::Vec::<usize>::new(),
        );
    }

    #[test]
    fn a_ragged_requested_tail_is_ignored() {
        let hashmap = names(10);
        let mut requested = std::vec::Vec::new();
        requested.extend_from_slice(&name(4));
        requested.extend_from_slice(&[0x00, 0x00]);
        assert_eq!(
            collect(serve_part_indices(&hashmap, 0, &requested)),
            std::vec![4],
        );
    }

    #[test]
    fn exhausting_segment_zero_plans_segment_one_and_holds_the_scope_at_zero() {
        let hashmap = names(150);
        let plan = plan_hashmap_update(&hashmap, 0, &name(73)).unwrap();
        assert_eq!(
            plan,
            HashmapUpdatePlan {
                segment: 1,
                entries_start: 74,
                entries_end: 148,
                scope_start: 0,
            },
        );
    }

    #[test]
    fn exhausting_segment_one_plans_the_partial_tail_and_slides_the_scope() {
        let hashmap = names(150);
        let plan = plan_hashmap_update(&hashmap, 0, &name(147)).unwrap();
        assert_eq!(
            plan,
            HashmapUpdatePlan {
                segment: 2,
                entries_start: 148,
                entries_end: 150,
                scope_start: 72,
            },
        );
    }

    #[test]
    fn a_last_name_off_the_segment_boundary_is_a_sequencing_refusal() {
        let hashmap = names(150);
        assert_eq!(
            plan_hashmap_update(&hashmap, 0, &name(10)).unwrap_err(),
            HashmapUpdatePlanError::NotOnSegmentBoundary,
        );
    }

    #[test]
    fn a_last_name_outside_the_scope_is_refused_by_name() {
        let hashmap = names(600);
        assert_eq!(
            plan_hashmap_update(&hashmap, 300, &name(73)).unwrap_err(),
            HashmapUpdatePlanError::LastMapHashNotInScope,
        );
        assert_eq!(
            plan_hashmap_update(&hashmap, 0, &name(295)).unwrap_err(),
            HashmapUpdatePlanError::LastMapHashNotInScope,
        );
    }
}
