use std::cell::Cell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Footprint {
    pub blocks: usize,
    pub bytes: usize,
}

thread_local! {
    static FOOTPRINT: Cell<Footprint> = const { Cell::new(Footprint { blocks: 0, bytes: 0 }) };
}

pub(super) fn allocate<T: 'static>(value: T) -> &'static mut T {
    FOOTPRINT.with(|counter| {
        let before = counter.get();
        counter.set(Footprint {
            blocks: before.blocks.checked_add(1).unwrap(),
            bytes: before.bytes.checked_add(std::mem::size_of::<T>()).unwrap(),
        });
    });
    Box::leak(Box::new(value))
}

pub(super) fn reset_accounting() {
    FOOTPRINT.with(|counter| {
        counter.set(Footprint {
            blocks: 0,
            bytes: 0,
        })
    });
}

pub(super) fn footprint() -> Footprint {
    FOOTPRINT.with(Cell::get)
}
