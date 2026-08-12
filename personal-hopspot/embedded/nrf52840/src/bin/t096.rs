#![no_std]
#![no_main]

use panic_halt as _;

compile_error!(
    "the Mesh Node T096 target is scaffold-only: complete its flash layout, KCT8103L control, ST7735 face, and UC6580 bring-up before enabling firmware"
);
