#![forbid(unsafe_code)]

mod desktop;
mod host_usb;

fn main() {
    desktop::run();
}
