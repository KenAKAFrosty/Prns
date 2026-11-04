//! Bounded, ordered catalog reads. A cursor is exclusive in one identity's key
//! space; each page has its own read snapshot. Mutations before a cursor appear
//! on refresh, rather than invalidating unrelated traversal with a global epoch.
use super::*;
use std::ops::Bound;

pub(super) struct AudiencePage {
    pub rows: Vec<([u8; 16], rows::Audience)>,
    pub next: Option<[u8; 16]>,
}

impl Read {
    pub(super) fn audience_page(
        &self,
        group: [u8; 32],
        after: Option<[u8; 16]>,
        limits: crate::PageLimits,
    ) -> Result<AudiencePage, EnrollmentError> {
        let (max_rows, max_bytes) = limits.checked().map_err(|_| EnrollmentError::Invalid)?;
        let slot = std::mem::size_of::<([u8; 16], rows::Audience)>();
        if max_bytes < slot {
            return Err(EnrollmentError::Invalid);
        }
        let lower = keys::audience(group, after.unwrap_or([0; 16]));
        let upper = keys::audience(group, [u8::MAX; 16]);
        let bound = if after.is_some() {
            Bound::Excluded(lower.as_slice())
        } else {
            Bound::Included(lower.as_slice())
        };
        let range = self
            .audience
            .range::<&[u8]>((bound, Bound::Included(upper.as_slice())))
            .map_err(db_error)?;
        let mut page = AudiencePage {
            rows: Vec::new(),
            next: None,
        };
        let mut last = None;
        for row in range {
            let (key, value) = row.map_err(db_error)?;
            let needed = page
                .rows
                .len()
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
            if page.rows.len() == max_rows || needed.checked_mul(slot).is_none_or(|n| n > max_bytes)
            {
                page.next = last;
                break;
            }
            let key: &keys::AudienceKey = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let recipient: [u8; 16] = key
                .get(32..)
                .ok_or(EnrollmentError::Corrupt)?
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let row: rows::Audience =
                rows::decode_row(value.value(), self.header.limits.row_bytes()?)?;
            if !row.admitted && row.offers == 0 {
                return Err(EnrollmentError::Corrupt);
            }
            if needed > page.rows.capacity() {
                page.rows
                    .try_reserve_exact(1)
                    .map_err(|_| EnrollmentError::Capacity)?;
            }
            if page
                .rows
                .capacity()
                .checked_mul(slot)
                .is_none_or(|n| n > max_bytes)
            {
                return Err(EnrollmentError::Capacity);
            }
            page.rows.push((recipient, row));
            last = Some(recipient);
        }
        Ok(page)
    }
}
