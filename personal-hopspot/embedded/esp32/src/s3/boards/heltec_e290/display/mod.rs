mod controller;

use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{
    gpio::{Input, Output},
    spi::master::Spi,
    Async,
};
use personal_hopspot_core::{
    display::{
        DisplayDuration, EinkPolicy, EinkPolicyConfiguration, EinkRefreshPolicy,
        PresentationOutcome, RefreshKind,
    },
    face_64x128::Frame,
};

use crate::s3::RetainedDisplayDevice;

use self::controller::{Controller, ControllerError};

const POWER_SETTLE_MS: u64 = 10;
const TELEMETRY_MINIMUM: DisplayDuration = match DisplayDuration::from_millis(30_000) {
    Ok(duration) => duration,
    Err(_) => panic!("the E290 telemetry spacing is nonzero"),
};
const RETAINED_POLICY: EinkPolicy = match EinkPolicy::new(EinkPolicyConfiguration {
    telemetry_minimum: TELEMETRY_MINIMUM,
    refresh: EinkRefreshPolicy::FullOnly,
}) {
    Ok(policy) => policy,
    Err(_) => panic!("a full-only policy has no maximum-age relationship"),
};

pub(crate) type DisplaySpi = ExclusiveDevice<Spi<'static, Async>, Output<'static>, Delay>;

pub(crate) const fn retained_policy() -> EinkPolicy {
    RETAINED_POLICY
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum E290DisplayError {
    Unavailable,
    UnsupportedRefresh(RefreshKind),
    Controller(ControllerError),
}

pub(crate) struct E290Display {
    controller: Option<Controller>,
    power: Output<'static>,
}

impl E290Display {
    pub(crate) fn new(
        spi: Option<DisplaySpi>,
        data_command: Output<'static>,
        reset: Output<'static>,
        busy: Input<'static>,
        power: Output<'static>,
    ) -> Self {
        Self {
            controller: spi.map(|spi| Controller::new(spi, data_command, reset, busy)),
            power,
        }
    }

    pub(crate) const fn is_available(&self) -> bool {
        self.controller.is_some()
    }

    async fn present_frame(
        &mut self,
        frame: &Frame,
        refresh: RefreshKind,
    ) -> Result<(), E290DisplayError> {
        if refresh != RefreshKind::Full {
            return Err(E290DisplayError::UnsupportedRefresh(refresh));
        }
        let controller = self
            .controller
            .as_mut()
            .ok_or(E290DisplayError::Unavailable)?;
        let started_at = Instant::now();
        self.power.set_high();
        Timer::after(Duration::from_millis(POWER_SETTLE_MS)).await;

        let result = async {
            controller
                .initialize()
                .await
                .map_err(E290DisplayError::Controller)?;
            controller
                .stream_frame(frame)
                .await
                .map_err(E290DisplayError::Controller)?;
            controller
                .activate()
                .await
                .map_err(E290DisplayError::Controller)?;
            controller
                .deep_sleep()
                .await
                .map_err(E290DisplayError::Controller)
        }
        .await;

        controller.assert_reset();
        self.power.set_low();
        log::info!(
            "E290 display refresh elapsed_ms={} result={result:?}",
            started_at.elapsed().as_millis()
        );
        result
    }
}

impl RetainedDisplayDevice for E290Display {
    async fn present(&mut self, frame: &Frame, refresh: RefreshKind) -> PresentationOutcome {
        match self.present_frame(frame, refresh).await {
            Ok(()) => PresentationOutcome::Succeeded,
            Err(error) => {
                log::error!("E290 display refresh failed: {error:?}");
                PresentationOutcome::Failed
            }
        }
    }
}
