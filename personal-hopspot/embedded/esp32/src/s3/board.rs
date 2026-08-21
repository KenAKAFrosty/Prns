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
    pub(crate) initialized: bool,
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

#[allow(async_fn_in_trait)]
pub(crate) trait Esp32S3Board {
    const ANNOUNCE_APP_DATA: &'static [u8];
    const NODE_ANNOUNCE_APP_DATA: &'static [u8];
    const BOOT_BANNER: &'static str;
    const USB_INTERFACE_ID: InterfaceId;
    const FLASH_LAYOUT: screen::HopspotS3FlashLayout;
    type Display: DrawTarget<Color = BinaryColor>;
    type Battery: screen::BatterySource;
    type Gnss: GnssProvider;

    fn flush(display: &mut Self::Display);
    fn set_display_awake(display: &mut Self::Display, awake: bool);
    async fn bringup(
        peripherals: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery, Self::Gnss>;
}
