use std::time::Duration;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics_simulator::{
    BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};

use personal_hopspot_ui as screen;

const PANEL: Size = Size::new(64, 128);
const FRAME: Duration = Duration::from_millis(33);

pub fn run() {
    let output = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::OledBlue)
        .scale(4)
        .build();
    let mut window = Window::new("Personal Hopspot", &output);
    let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);

    // The flip: later we spawn `Prns::run(platform)` as a side task here, and this
    // window loop is just our app.
    loop {
        screen::splash(&mut display, "Desktop");
        window.update(&display);
        if window.events().any(|event| event == SimulatorEvent::Quit) {
            return;
        }
        std::thread::sleep(FRAME);
    }
}
