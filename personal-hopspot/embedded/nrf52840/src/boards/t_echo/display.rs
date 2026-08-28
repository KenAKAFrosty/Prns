use personal_hopspot_core::face_64x128::{Frame, PanelTransform};

use crate::retained_display::{RetainedDisplayDevice, RetainedRefresh};

use super::hardware::TechoEink;
use super::raster::{rasterize_row, transform};
use super::ssd1681::Ssd1681Error;

pub(crate) struct TechoDisplayDevice {
    driver: TechoEink,
    transform: PanelTransform,
}

impl TechoDisplayDevice {
    pub(crate) fn new(driver: TechoEink) -> Self {
        Self {
            driver,
            transform: transform(),
        }
    }
}

impl RetainedDisplayDevice for TechoDisplayDevice {
    type Error = Ssd1681Error;

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
