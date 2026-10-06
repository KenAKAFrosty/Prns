/// The last confirmed contents of a monochrome panel's page-addressed RAM.
pub struct MonochromePageCache<const WIDTH: usize, const PAGES: usize> {
    pages: [[u8; WIDTH]; PAGES],
    known: bool,
}

impl<const WIDTH: usize, const PAGES: usize> MonochromePageCache<WIDTH, PAGES> {
    pub const fn new() -> Self {
        Self {
            pages: [[0; WIDTH]; PAGES],
            known: false,
        }
    }

    pub fn invalidate(&mut self) {
        self.known = false;
    }

    /// A failed transfer may have partially reached the panel, so retry the whole image.
    pub fn present<E>(
        &mut self,
        mut map_page: impl FnMut(usize) -> [u8; WIDTH],
        mut write_page: impl FnMut(usize, &[u8; WIDTH]) -> Result<(), E>,
    ) -> Result<(), E> {
        for page in 0..PAGES {
            let next = map_page(page);
            if self.known && next == self.pages[page] {
                continue;
            }
            self.pages[page] = next;
            if let Err(error) = write_page(page, &next) {
                self.invalidate();
                return Err(error);
            }
        }
        self.known = true;
        Ok(())
    }
}

impl<const WIDTH: usize, const PAGES: usize> Default for MonochromePageCache<WIDTH, PAGES> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[test]
    fn unchanged_frames_send_nothing_and_single_page_changes_send_one_page() {
        let mut cache = MonochromePageCache::<128, 8>::new();
        let mut transferred = Vec::new();
        let mut image = [[0; 128]; 8];
        for expected in [8, 0, 1] {
            if expected == 1 {
                image[3][17] = 0x80;
            }
            transferred.clear();
            cache
                .present(
                    |page| image[page],
                    |page, bytes| {
                        transferred.push((page, *bytes));
                        Ok::<_, ()>(())
                    },
                )
                .unwrap();
            assert_eq!(transferred.len(), expected);
            if expected == 1 {
                assert_eq!(transferred, [(3, image[3])]);
            }
        }
    }

    #[test]
    fn partially_written_pages_are_all_repaired_after_any_failed_transfer() {
        for failed_page in 0..8 {
            let mut cache = MonochromePageCache::<128, 8>::new();
            let mut panel = [[0; 128]; 8];
            cache.present(|_| [0; 128], |_, _| Ok::<_, ()>(())).unwrap();
            let image = [[0xff; 128]; 8];
            let result = cache.present(
                |page| image[page],
                |page, bytes| {
                    if page == failed_page {
                        panel[page][..13].copy_from_slice(&bytes[..13]);
                        return Err(());
                    }
                    panel[page] = *bytes;
                    Ok(())
                },
            );
            assert_eq!(result, Err(()));
            assert_ne!(panel, image);
            let mut retried = Vec::new();
            cache
                .present(
                    |page| image[page],
                    |page, bytes| {
                        retried.push(page);
                        panel[page] = *bytes;
                        Ok::<_, ()>(())
                    },
                )
                .unwrap();
            assert_eq!(retried, (0..8).collect::<Vec<_>>());
            assert_eq!(panel, image);
        }
    }

    #[test]
    fn lost_panel_memory_forces_redraw_of_an_unchanged_image() {
        let mut cache = MonochromePageCache::<128, 8>::new();
        cache.present(|_| [0; 128], |_, _| Ok::<_, ()>(())).unwrap();
        cache.invalidate();
        let mut writes = 0;
        cache
            .present(
                |_| [0; 128],
                |_, _| {
                    writes += 1;
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        assert_eq!(writes, 8);
    }
}
