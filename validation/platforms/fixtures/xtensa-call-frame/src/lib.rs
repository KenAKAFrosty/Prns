#![no_std]

extern "C" {
    fn fifteen_arguments(
        a0: u32, a1: u32, a2: u32, a3: u32, a4: u32,
        a5: u32, a6: u32, a7: u32, a8: u32, a9: u32,
        a10: u32, a11: u32, a12: u32, a13: u32, a14: u32,
    ) -> u32;
}

/// Fifteen words use six register arguments and nine outgoing stack words.
/// No variable-size arrays, allocation, assembly, or scheduler is involved.
#[no_mangle]
pub unsafe extern "C" fn call_fifteen_arguments(input: *const u32) -> u32 {
    fifteen_arguments(
        input.add(0).read(), input.add(1).read(), input.add(2).read(),
        input.add(3).read(), input.add(4).read(), input.add(5).read(),
        input.add(6).read(), input.add(7).read(), input.add(8).read(),
        input.add(9).read(), input.add(10).read(), input.add(11).read(),
        input.add(12).read(), input.add(13).read(), input.add(14).read(),
    )
}
