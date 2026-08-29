# Prns vendor record

- Package: `esp-wifi-sys-esp32s3 0.2.0`
- Upstream: `https://github.com/esp-rs/esp-wifi-sys`
- Upstream revision: `fee9770fc96fa3bb753b2ce4bd968daa4f068a04`
- Radio sources: ESP-IDF 5.5.3 (`2c211b236707889e8400c4dc5644dd5c4ee071e0`)
- Package license: `MIT OR Apache-2.0`
- Added Mbed TLS objects: revision `ffb280bb63c78bfec1e1ab55040671768c85c923`,
  `Apache-2.0 OR GPL-2.0-or-later` with the `Apache-2.0` alternative selected
  and reproduced at `release/licenses/mbedtls-Apache-2.0.txt`
- Local changes:
  - Rebuild `libwpa_supplicant.a` for ESP32-S3 with WPA3-SAE, SAE H2E, and
    SoftAP-SAE enabled.
  - Use the software mbedTLS crypto backend and link the resulting
    `libmbedcrypto.a`; ESP-IDF hardware AES, MPI, SHA, and ECC acceleration are
    disabled to keep the Rust-side integration independent of ESP-IDF hardware
    crypto allocator and locking APIs.
  - Link `libmbedcrypto.a` from the package build script.
  - Isolate the package as its own Cargo workspace for repository validation.

The complete build recipe is committed under `prns-build/`. It pins the
ESP-IDF and Mbed TLS source revisions, Espressif compiler identity, complete
Kconfig input, and output hashes. Rebuild and compare the archives with:

```console
./tools/prns build esp32s3-wpa3-radio
```

The material Kconfig values were:

```text
CONFIG_ESP_WIFI_ENABLE_WPA3_SAE=y
CONFIG_ESP_WIFI_ENABLE_SAE_H2E=y
CONFIG_ESP_WIFI_SOFTAP_SAE_SUPPORT=y
CONFIG_ESP_WIFI_MBEDTLS_CRYPTO=y
CONFIG_MBEDTLS_INTERNAL_MEM_ALLOC=y
# CONFIG_ESP_WIFI_ENABLE_SAE_PK is not set
# CONFIG_MBEDTLS_HARDWARE_AES is not set
# CONFIG_MBEDTLS_HARDWARE_MPI is not set
# CONFIG_MBEDTLS_HARDWARE_SHA is not set
# CONFIG_MBEDTLS_HARDWARE_ECC is not set
```

Archive identities are authoritative in `prns-build/identity.env`. The build
uses ESP-IDF's supported `esp-14.2.0_20251107` Xtensa GCC toolchain and refuses
modified or mismatched source checkouts, compiler identities, configurations,
or outputs.

Both station and SoftAP SAE are deliberate in this S3 artifact: outbound WPA3
is the product requirement, while SoftAP SAE preserves the independently
hardware-proven capability. OWE, DPP, SAE-PK, and the mbedTLS TLS client remain
disabled.

The release-profile Heltec V4 firmware footprint measured on 2026-08-25 with
`cargo heltec-v4` and `xtensa-esp32s3-elf-size` changed as follows relative to
the pristine S3 archive import:

```text
section   pristine     WPA3-SAE     delta
text      1,750,763    1,815,731    +64,968
data         33,992       34,008        +16
bss         643,624      643,624          0
```

That is 64,984 additional flash-resident bytes (`text + data`), or 3.64% of
the pristine firmware's flash-resident size, with no measured static-RAM
increase. This records the cost of retaining both station and SoftAP SAE in the
shared S3 artifact rather than hiding it in the archive-size change.

On 2026-08-25 this spike was exercised on an ESP32-S3 (revision 0.2) SoftAP and
a Pixel 10 Pro. The client observed `[RSN-SAE-CCMP-128][MFPR][MFPC]`, completed
the SAE handshake, associated as WPA3 security type 4, received `192.168.4.2`
over DHCP, and reached the captive portal. A second association succeeded after
an explicit board reset with driver diagnostic logging disabled.
