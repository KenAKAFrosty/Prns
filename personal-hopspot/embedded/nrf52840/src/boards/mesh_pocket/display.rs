use personal_hopspot_core::display::{
    DisplayDuration, EinkPolicy, EinkPolicyConfiguration, EinkRefreshPolicy, PartialRefreshLimit,
};
use personal_hopspot_core::face_64x128::{Frame, PanelTransform};

use crate::retained_display::{RetainedDisplayDevice, RetainedRefresh};

use super::hardware::MeshPocketEink;
use super::raster::{rasterize_row, transform};
use super::ssd1680::Ssd1680Error;

const PARTIAL_REFRESH_LIMIT: u32 = 10;
const FULL_REFRESH_MAXIMUM_AGE_MS: u64 = 30 * 60 * 1_000;
const TELEMETRY_MINIMUM_INTERVAL_MS: u64 = 30_000;

pub(crate) fn retained_policy() -> EinkPolicy {
    EinkPolicy::new(EinkPolicyConfiguration {
        telemetry_minimum: DisplayDuration::from_millis(TELEMETRY_MINIMUM_INTERVAL_MS)
            .expect("MeshPocket telemetry spacing is nonzero"),
        refresh: EinkRefreshPolicy::Partial {
            maximum_consecutive: PartialRefreshLimit::new(PARTIAL_REFRESH_LIMIT)
                .expect("MeshPocket partial refresh limit is nonzero"),
            full_maximum_age: DisplayDuration::from_millis(FULL_REFRESH_MAXIMUM_AGE_MS)
                .expect("MeshPocket full refresh age is nonzero"),
        },
    })
    .expect("MeshPocket telemetry spacing does not exceed its full refresh age")
}

pub(crate) struct MeshPocketDisplayDevice {
    driver: MeshPocketEink,
    transform: PanelTransform,
}

impl MeshPocketDisplayDevice {
    pub(crate) fn new(driver: MeshPocketEink) -> Self {
        Self {
            driver,
            transform: transform(),
        }
    }
}

impl RetainedDisplayDevice for MeshPocketDisplayDevice {
    type Error = Ssd1680Error;

    async fn present(
        &mut self,
        frame: &Frame,
        refresh: RetainedRefresh,
    ) -> Result<(), Self::Error> {
        let transform = &self.transform;
        let mut rows = |y| rasterize_row(frame, transform, y);
        match refresh {
            RetainedRefresh::Full => self.driver.full_update(&mut rows).await,
            RetainedRefresh::Partial => self.driver.partial_update(&mut rows).await,
        }
    }

    async fn recover(&mut self) -> Result<(), Self::Error> {
        self.driver.recover().await
    }

    async fn deep_sleep(&mut self) -> Result<(), Self::Error> {
        self.driver.deep_sleep().await
    }
}
