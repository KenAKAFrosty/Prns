//! Card detail screen: Options + paginated status + Back/Next.

use crate::screen::render::layout::{
    CARD_TOP, HEIGHT, MENU_BACKING_H, MENU_DETAIL_STEP, MENU_ITEM_STEP,
};

/// Focusable controls on the interface detail screen (status lines are display-only).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterfaceDetailFocus {
    Options,
    Next,
    Back,
}

/// Layout without the old "Menu" subtitle so status gets vertical room.
pub(in crate::screen) const DETAIL_DIVIDER_Y: i32 = CARD_TOP + 14;
pub(in crate::screen) const DETAIL_OPTIONS_Y: i32 = CARD_TOP + 20;
pub(in crate::screen) const DETAIL_STATUS_TOP: i32 = DETAIL_OPTIONS_Y + MENU_ITEM_STEP;
/// Reserved bottom row for Next on non-final pages.
pub(in crate::screen) const DETAIL_CONTROL_Y: i32 = HEIGHT - MENU_ITEM_STEP;

#[must_use]
pub(in crate::screen) const fn status_slots_above_bottom_control() -> usize {
    let span = DETAIL_CONTROL_Y - DETAIL_STATUS_TOP;
    if span <= 0 {
        0
    } else {
        (span / MENU_DETAIL_STEP) as usize
    }
}

#[must_use]
pub(in crate::screen) fn interface_detail_status_line_count(
    detail_rows: usize,
    has_failure_reason: bool,
) -> usize {
    // Mode is always the first status line; failure adds a compact Fail: block.
    1 + detail_rows + usize::from(has_failure_reason) * 2
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) struct InterfaceDetailPage {
    pub status_start: usize,
    pub status_count: usize,
    pub shows_next: bool,
    pub shows_back: bool,
}

#[must_use]
fn back_fits_after_status(status_count: usize) -> bool {
    let back_y = DETAIL_STATUS_TOP + status_count as i32 * MENU_DETAIL_STEP;
    back_y + MENU_BACKING_H as i32 <= HEIGHT
}

/// Pack status lines into pages with Next on overflow and Back on the final page.
#[must_use]
pub(in crate::screen) fn interface_detail_page_count(status_lines: usize) -> usize {
    interface_detail_pages(status_lines).len().max(1)
}

#[must_use]
pub(in crate::screen) fn interface_detail_page(
    status_lines: usize,
    page: usize,
) -> InterfaceDetailPage {
    let pages = interface_detail_pages(status_lines);
    let index = page.min(pages.len().saturating_sub(1));
    pages.get(index).copied().unwrap_or(InterfaceDetailPage {
        status_start: 0,
        status_count: 0,
        shows_next: false,
        shows_back: true,
    })
}

#[must_use]
fn interface_detail_pages(status_lines: usize) -> heapless::Vec<InterfaceDetailPage, 16> {
    let mut pages = heapless::Vec::new();
    let per_with_next = status_slots_above_bottom_control().max(1);
    let mut offset = 0usize;

    loop {
        let remaining = status_lines.saturating_sub(offset);
        if remaining == 0 && !pages.is_empty() {
            break;
        }
        if remaining <= per_with_next && back_fits_after_status(remaining) {
            let _ = pages.push(InterfaceDetailPage {
                status_start: offset,
                status_count: remaining,
                shows_next: false,
                shows_back: true,
            });
            break;
        }
        let take = remaining.min(per_with_next);
        if take == 0 {
            let _ = pages.push(InterfaceDetailPage {
                status_start: offset,
                status_count: 0,
                shows_next: false,
                shows_back: true,
            });
            break;
        }
        let _ = pages.push(InterfaceDetailPage {
            status_start: offset,
            status_count: take,
            shows_next: true,
            shows_back: false,
        });
        offset = offset.saturating_add(take);
        if pages.is_full() {
            break;
        }
    }

    if pages.is_empty() {
        let _ = pages.push(InterfaceDetailPage {
            status_start: 0,
            status_count: 0,
            shows_next: false,
            shows_back: true,
        });
    }
    pages
}

#[must_use]
pub(in crate::screen) fn interface_detail_focus_cycle(
    page: InterfaceDetailPage,
) -> heapless::Vec<InterfaceDetailFocus, 3> {
    let mut cycle = heapless::Vec::new();
    let _ = cycle.push(InterfaceDetailFocus::Options);
    if page.shows_next {
        let _ = cycle.push(InterfaceDetailFocus::Next);
    }
    if page.shows_back {
        let _ = cycle.push(InterfaceDetailFocus::Back);
    }
    cycle
}

#[must_use]
pub(in crate::screen) fn clamp_interface_detail_focus(
    focus: InterfaceDetailFocus,
    page: InterfaceDetailPage,
) -> InterfaceDetailFocus {
    let cycle = interface_detail_focus_cycle(page);
    if cycle.contains(&focus) {
        focus
    } else {
        InterfaceDetailFocus::Options
    }
}

#[must_use]
pub(in crate::screen) fn next_interface_detail_focus(
    focus: InterfaceDetailFocus,
    page: InterfaceDetailPage,
) -> InterfaceDetailFocus {
    let cycle = interface_detail_focus_cycle(page);
    if cycle.is_empty() {
        return InterfaceDetailFocus::Options;
    }
    let Some(index) = cycle.iter().position(|item| *item == focus) else {
        return cycle[0];
    };
    cycle[(index + 1) % cycle.len()]
}
