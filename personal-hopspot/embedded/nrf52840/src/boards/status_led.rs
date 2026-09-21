use embassy_nrf::gpio::Output;

enum Polarity {
    #[cfg(any(
        feature = "board-t096",
        feature = "board-t1000e",
        feature = "board-rak4631"
    ))]
    ActiveHigh,
    #[cfg(any(feature = "board-t114", feature = "board-mesh-tower-v2"))]
    ActiveLow,
}

pub(crate) struct StatusLed {
    output: Output<'static>,
    polarity: Polarity,
}

impl StatusLed {
    #[cfg(any(
        feature = "board-t096",
        feature = "board-t1000e",
        feature = "board-rak4631"
    ))]
    pub(crate) fn active_high(output: Output<'static>) -> Self {
        Self {
            output,
            polarity: Polarity::ActiveHigh,
        }
    }

    #[cfg(any(feature = "board-t114", feature = "board-mesh-tower-v2"))]
    pub(crate) fn active_low(output: Output<'static>) -> Self {
        Self {
            output,
            polarity: Polarity::ActiveLow,
        }
    }

    pub(crate) fn illuminate(&mut self) {
        match self.polarity {
            #[cfg(any(
                feature = "board-t096",
                feature = "board-t1000e",
                feature = "board-rak4631"
            ))]
            Polarity::ActiveHigh => self.output.set_high(),
            #[cfg(any(feature = "board-t114", feature = "board-mesh-tower-v2"))]
            Polarity::ActiveLow => self.output.set_low(),
        }
    }

    pub(crate) fn extinguish(&mut self) {
        match self.polarity {
            #[cfg(any(
                feature = "board-t096",
                feature = "board-t1000e",
                feature = "board-rak4631"
            ))]
            Polarity::ActiveHigh => self.output.set_low(),
            #[cfg(any(feature = "board-t114", feature = "board-mesh-tower-v2"))]
            Polarity::ActiveLow => self.output.set_high(),
        }
    }

    /// Two short flashes so a headless board shows it reached the runtime, then heartbeat.
    #[cfg(feature = "board-rak4631")]
    pub(crate) async fn boot_splash(&mut self) {
        use embassy_time::Timer;
        for _ in 0..2 {
            self.illuminate();
            Timer::after_millis(100).await;
            self.extinguish();
            Timer::after_millis(100).await;
        }
    }
}
