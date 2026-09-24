use embassy_nrf::gpio::Output;

enum Polarity {
    #[cfg(any(
        feature = "board-t096",
        any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
        any(feature = "board-rak4631", feature = "board-rak10724"),
        feature = "board-wio-tracker-l1"
    ))]
    ActiveHigh,
    #[cfg(any(
        feature = "board-t114",
        feature = "board-mesh-tower-v2",
        feature = "board-muzi-base-duo"
    ))]
    ActiveLow,
}

pub(crate) struct StatusLed {
    output: Output<'static>,
    polarity: Polarity,
}

impl StatusLed {
    #[cfg(any(
        feature = "board-t096",
        any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
        any(feature = "board-rak4631", feature = "board-rak10724"),
        feature = "board-wio-tracker-l1"
    ))]
    pub(crate) fn active_high(output: Output<'static>) -> Self {
        Self {
            output,
            polarity: Polarity::ActiveHigh,
        }
    }

    #[cfg(any(
        feature = "board-t114",
        feature = "board-mesh-tower-v2",
        feature = "board-muzi-base-duo"
    ))]
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
                any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
                any(feature = "board-rak4631", feature = "board-rak10724"),
                feature = "board-wio-tracker-l1"
            ))]
            Polarity::ActiveHigh => self.output.set_high(),
            #[cfg(any(
                feature = "board-t114",
                feature = "board-mesh-tower-v2",
                feature = "board-muzi-base-duo"
            ))]
            Polarity::ActiveLow => self.output.set_low(),
        }
    }

    pub(crate) fn extinguish(&mut self) {
        match self.polarity {
            #[cfg(any(
                feature = "board-t096",
                any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
                any(feature = "board-rak4631", feature = "board-rak10724"),
                feature = "board-wio-tracker-l1"
            ))]
            Polarity::ActiveHigh => self.output.set_low(),
            #[cfg(any(
                feature = "board-t114",
                feature = "board-mesh-tower-v2",
                feature = "board-muzi-base-duo"
            ))]
            Polarity::ActiveLow => self.output.set_high(),
        }
    }

    /// Two short flashes make successful runtime entry visible on the headless RAK4631.
    #[cfg(any(feature = "board-rak4631", feature = "board-rak10724"))]
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
