use embedded_graphics::mono_font::iso_8859_1::FONT_4X6;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};

use crate::screen::state::groups::{DiscoveryGroupEditor, GroupEditorScreen};

use super::super::layout::*;
use super::{draw_menu_header, draw_menu_item};

const ROW_TOP: i32 = MENU_ITEM_TOP;
const VISIBLE_ROWS: usize = 6;

pub(in crate::screen) fn draw_group_editor<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    editor: DiscoveryGroupEditor,
) {
    match editor.screen {
        GroupEditorScreen::List { cursor } => {
            draw_menu_header(display, "Groups", "Membership");
            let names = editor.names();
            let item_count = names.len() + usize::from(editor.can_add()) + 2;
            let start = cursor
                .saturating_sub(VISIBLE_ROWS - 1)
                .min(item_count.saturating_sub(VISIBLE_ROWS));
            for visible in 0..VISIBLE_ROWS {
                let index = start + visible;
                if index >= item_count {
                    break;
                }
                let label = if index < names.len() {
                    names[index].as_str()
                } else if editor.can_add() && index == names.len() {
                    "Add"
                } else if index + 1 == item_count {
                    "Back"
                } else {
                    "Save"
                };
                draw_menu_item(
                    display,
                    ROW_TOP + visible as i32 * MENU_ITEM_STEP,
                    label,
                    index == cursor,
                );
            }
        }
        GroupEditorScreen::Item { cursor, .. } => {
            draw_menu_header(display, "Groups", editor.active_name().unwrap_or("Group"));
            for (index, label) in ["Edit", "Remove", "Back"].iter().enumerate() {
                draw_menu_item(
                    display,
                    ROW_TOP + index as i32 * MENU_ITEM_STEP,
                    label,
                    index == cursor,
                );
            }
        }
        GroupEditorScreen::Text { .. } => {
            draw_menu_header(display, "Groups", "Edit name");
            let name = editor.active_name().unwrap_or("");
            let shown = if name.len() > 20 {
                name.get(name.len() - 20..).unwrap_or(name)
            } else {
                name
            };
            draw_menu_item(display, ROW_TOP, shown, false);
            draw_menu_item(
                display,
                ROW_TOP + 2 * MENU_ITEM_STEP,
                editor.text_choice().unwrap_or("Done"),
                true,
            );
            let hint = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
            let _ = Text::with_baseline(
                "Tap choose / Hold use",
                Point::new(2, ROW_TOP + 4 * MENU_ITEM_STEP),
                hint,
                Baseline::Top,
            )
            .draw(display);
        }
    }
}
