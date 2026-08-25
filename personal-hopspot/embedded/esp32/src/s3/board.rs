use super::*;

#[cfg(feature = "lora")]
pub(crate) type LoraRadio = Sx126x<
    ExclusiveDevice<Spi<'static, esp_hal::Async>, Output<'static>, Delay>,
    Input<'static>,
    Input<'static>,
    Output<'static>,
    Delay,
>;

pub(crate) struct BoardDisplay<D> {
    pub(crate) device: D,
    /// Whether bring-up produced a usable display path. Controller initialization,
    /// sleep, and rail state remain private to the board presenter.
    pub(crate) available: bool,
}

pub(crate) struct BoardFace<D, B> {
    pub(crate) display: BoardDisplay<D>,
    pub(crate) battery: B,
    pub(crate) button: Input<'static>,
}

pub(crate) struct S3InterfaceHardware {
    pub(crate) usb_device: USB_DEVICE<'static>,
    #[cfg(feature = "lora")]
    pub(crate) lora_radio: LoraRadio,
    pub(crate) wifi: esp_hal::peripherals::WIFI<'static>,
    pub(crate) bluetooth: esp_hal::peripherals::BT<'static>,
}

pub(crate) struct S3ManifoldHardware {
    pub(crate) cpu_control: esp_hal::peripherals::CPU_CTRL<'static>,
    pub(crate) software_interrupt: esp_hal::interrupt::software::SoftwareInterrupt<'static, 1>,
    pub(crate) timebase: EmbassyTimebase,
    pub(crate) rtc: esp_hal::rtc_cntl::Rtc<'static>,
}

pub(crate) struct S3BoardHardware<D, B, G> {
    pub(crate) face: BoardFace<D, B>,
    pub(crate) gnss: G,
    pub(crate) interface_hardware: S3InterfaceHardware,
    pub(crate) manifold: S3ManifoldHardware,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplayIoError {
    Presentation,
    Blanking,
}

pub(crate) fn write_face_to_draw_target<D>(display: &mut D, frame: &screen::face_64x128::Frame)
where
    D: DrawTarget<Color = BinaryColor>,
{
    let pixels = (0..screen::face_64x128::LOGICAL_HEIGHT).flat_map(|y| {
        (0..screen::face_64x128::LOGICAL_WIDTH).map(move |x| {
            let point = embedded_graphics::geometry::Point::new(x as i32, y as i32);
            let color = if frame.pixel_is_on(point) {
                BinaryColor::On
            } else {
                BinaryColor::Off
            };
            embedded_graphics::Pixel(point, color)
        })
    });
    let _ = display.draw_iter(pixels);
}

#[allow(async_fn_in_trait)]
pub(crate) trait Esp32S3Board {
    const ANNOUNCE_APP_DATA: &'static [u8];
    const NODE_ANNOUNCE_APP_DATA: &'static [u8];
    const BOOT_BANNER: &'static str;
    const USB_INTERFACE_ID: InterfaceId;
    const FLASH_LAYOUT: screen::HopspotS3FlashLayout;
    /// User-visible blanking capability, independent of display availability
    /// and the presenter's internal controller power state.
    const USER_BLANKING: screen::UserBlanking;
    type Display;
    /// Board-local presentation failures retain controller phase detail.
    type DisplayError: core::fmt::Debug;
    /// Compile-time-selected frame/planner owner; immediate displays keep one
    /// frame while retained displays opt into exact two-frame ownership.
    type Presentation: S3PresentationState;
    type Battery: screen::BatterySource;
    type Gnss: GnssProvider;

    fn presentation() -> Self::Presentation;
    /// Present the frozen candidate using the waveform selected by the shared
    /// planner. Long-running controller phases must await rather than block.
    async fn present(
        display: &mut Self::Display,
        frame: &screen::face_64x128::Frame,
        kind: screen::presentation::RefreshKind,
    ) -> Result<(), Self::DisplayError>;
    fn set_display_awake(
        display: &mut Self::Display,
        awake: bool,
    ) -> Result<(), Self::DisplayError>;
    async fn bringup(
        peripherals: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery, Self::Gnss>;
}
