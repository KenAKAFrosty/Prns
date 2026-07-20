use clap::ValueEnum;

pub(crate) const T_ECHO_BASE: &str = "0x27000";
pub(crate) const T_ECHO_FAMILY: &str = "0xADA52840";
pub(crate) const ESP32S3_TARGET: &str = "xtensa-esp32s3-none-elf";
const ESP32C6_TARGET: &str = "riscv32imac-unknown-none-elf";
const HELTEC_V4_FIRMWARE: &str = "hopspot-heltec-v4";
const HELTEC_V4_ARTIFACT: &str = "hopspot-heltec-v4.bin";
const T_BEAM_SUPREME_FIRMWARE: &str = "hopspot-t-beam-supreme";
const T_BEAM_SUPREME_ARTIFACT: &str = "hopspot-t-beam-supreme.bin";
const XIAO_ESP32_C6_FIRMWARE: &str = "hopspot-xiao-esp32-c6";
const XIAO_ESP32_C6_ARTIFACT: &str = "hopspot-xiao-esp32-c6.bin";
const ESP_PARTITIONS_8MB: &str = "partitions-hopspot-8mb.csv";
const ESP_PARTITIONS_4MB: &str = "partitions-hopspot-4mb.csv";

#[derive(Clone, Copy, PartialEq, ValueEnum)]
pub(crate) enum BoardId {
    HeltecV4,
    TBeamSupreme,
    XiaoEsp32C6,
    TEcho,
}

impl BoardId {
    pub(crate) fn target(self) -> &'static BoardTarget {
        match self {
            BoardId::TEcho => &BOARDS[0],
            BoardId::HeltecV4 => &BOARDS[1],
            BoardId::TBeamSupreme => &BOARDS[2],
            BoardId::XiaoEsp32C6 => &BOARDS[3],
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum BoardBackend {
    TEchoUf2,
    EspFlash(&'static EspImageSpec),
}

impl BoardBackend {
    pub(crate) fn ready(self) -> bool {
        true
    }
}

pub(crate) struct BoardTarget {
    pub(crate) slug: &'static str,
    pub(crate) name: &'static str,
    pub(crate) silicon: &'static str,
    pub(crate) interfaces: &'static [&'static str],
    pub(crate) backend: BoardBackend,
}

impl BoardTarget {
    pub(crate) fn supports_wifi_config(&self) -> bool {
        self.interfaces.contains(&"Wi-Fi Auto")
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct EspImageSpec {
    pub(crate) chip: &'static str,
    pub(crate) chip_family: &'static str,
    pub(crate) flash_size: &'static str,
    pub(crate) target: &'static str,
    pub(crate) partition_table: &'static str,
    pub(crate) package: &'static str,
    pub(crate) binary: &'static str,
    pub(crate) artifact: &'static str,
    pub(crate) web_name: &'static str,
    pub(crate) wifi_configurable: bool,
    pub(crate) after_reset: &'static str,
}

pub(crate) const HELTEC_V4_ESP: EspImageSpec = EspImageSpec {
    chip: "esp32s3",
    chip_family: "ESP32-S3",
    flash_size: "8mb",
    target: ESP32S3_TARGET,
    partition_table: ESP_PARTITIONS_8MB,
    package: HELTEC_V4_FIRMWARE,
    binary: HELTEC_V4_FIRMWARE,
    artifact: HELTEC_V4_ARTIFACT,
    web_name: "Hopspot Heltec V4",
    wifi_configurable: true,
    after_reset: "watchdog-reset",
};

pub(crate) const T_BEAM_SUPREME_ESP: EspImageSpec = EspImageSpec {
    chip: "esp32s3",
    chip_family: "ESP32-S3",
    flash_size: "8mb",
    target: ESP32S3_TARGET,
    partition_table: ESP_PARTITIONS_8MB,
    package: T_BEAM_SUPREME_FIRMWARE,
    binary: T_BEAM_SUPREME_FIRMWARE,
    artifact: T_BEAM_SUPREME_ARTIFACT,
    web_name: "Hopspot T-Beam Supreme",
    wifi_configurable: true,
    after_reset: "watchdog-reset",
};

pub(crate) const XIAO_ESP32_C6_ESP: EspImageSpec = EspImageSpec {
    chip: "esp32c6",
    chip_family: "ESP32-C6",
    flash_size: "4mb",
    target: ESP32C6_TARGET,
    partition_table: ESP_PARTITIONS_4MB,
    package: XIAO_ESP32_C6_FIRMWARE,
    binary: XIAO_ESP32_C6_FIRMWARE,
    artifact: XIAO_ESP32_C6_ARTIFACT,
    web_name: "Hopspot XIAO ESP32-C6",
    wifi_configurable: false,
    after_reset: "hard-reset",
};

const T_ECHO: BoardTarget = BoardTarget {
    slug: "t-echo",
    name: "LilyGO T-Echo",
    silicon: "nRF52840 + SX1262",
    interfaces: &["BLE Auto", "LoRa", "USB Auto"],
    backend: BoardBackend::TEchoUf2,
};

pub(crate) const BOARDS: &[BoardTarget] = &[
    T_ECHO,
    BoardTarget {
        slug: "heltec-v4",
        name: "Heltec V4",
        silicon: "ESP32-S3 + SX1262",
        interfaces: &["Wi-Fi Auto", "BLE Auto", "LoRa", "ESP-NOW", "USB Auto"],
        backend: BoardBackend::EspFlash(&HELTEC_V4_ESP),
    },
    BoardTarget {
        slug: "t-beam-supreme",
        name: "LilyGO T-Beam Supreme",
        silicon: "ESP32-S3 + SX1262",
        interfaces: &["Wi-Fi Auto", "BLE Auto", "LoRa", "ESP-NOW", "USB Auto"],
        backend: BoardBackend::EspFlash(&T_BEAM_SUPREME_ESP),
    },
    BoardTarget {
        slug: "xiao-esp32-c6",
        name: "Seeed Studio XIAO ESP32-C6",
        silicon: "ESP32-C6 + SX1262",
        interfaces: &["ESP-NOW", "BLE Auto", "USB Auto"],
        backend: BoardBackend::EspFlash(&XIAO_ESP32_C6_ESP),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_esp_board_selects_its_own_firmware_package() {
        let expected = [
            (BoardId::HeltecV4, "heltec-v4", "hopspot-heltec-v4"),
            (
                BoardId::TBeamSupreme,
                "t-beam-supreme",
                "hopspot-t-beam-supreme",
            ),
            (
                BoardId::XiaoEsp32C6,
                "xiao-esp32-c6",
                "hopspot-xiao-esp32-c6",
            ),
        ];

        for (board, slug, firmware) in expected {
            let target = board.target();
            let package_and_binary = match target.backend {
                BoardBackend::EspFlash(spec) => Some((spec.package, spec.binary)),
                BoardBackend::TEchoUf2 => None,
            };
            assert_eq!(
                (target.slug, package_and_binary),
                (slug, Some((firmware, firmware)))
            );
        }
    }
}
