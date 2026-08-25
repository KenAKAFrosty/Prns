use embassy_nrf::gpio::Output;

enum Polarity {
    #[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
    ActiveHigh,
    #[cfg(any(feature = "board-t114", feature = "board-mesh-tower-v2"))]
    ActiveLow,
}

pub(crate) struct StatusLed {
    output: Output<'static>,
    polarity: Polarity,
}

impl StatusLed {
    #[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
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
            #[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
            Polarity::ActiveHigh => self.output.set_high(),
            #[cfg(any(feature = "board-t114", feature = "board-mesh-tower-v2"))]
            Polarity::ActiveLow => self.output.set_low(),
        }
    }

    pub(crate) fn extinguish(&mut self) {
        match self.polarity {
            #[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
            Polarity::ActiveHigh => self.output.set_low(),
            #[cfg(any(feature = "board-t114", feature = "board-mesh-tower-v2"))]
            Polarity::ActiveLow => self.output.set_high(),
        }
    }
}
