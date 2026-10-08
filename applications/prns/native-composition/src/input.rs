//! Semantic bounds enforced by the application-owned typed commands.
pub(crate) const MAX_PATH_BYTES: usize = 4 * 1024;
pub(crate) const MAX_INPUT_BYTES: usize = 64 * 1024;
pub(crate) const MAX_RESTORATION_IDENTIFIER_BYTES: usize = 1024;

pub(crate) fn bounded_text(fields: &[&str]) -> bool {
    fields
        .iter()
        .fold(0_usize, |size, field| size.saturating_add(field.len()))
        <= MAX_INPUT_BYTES
}
