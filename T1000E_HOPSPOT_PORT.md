# Prns Hopspot port: Seeed SenseCAP T1000-E (nRF52840 + LR1110)

Plan to contribute a T1000-E "Personal Hopspot" (on-device Prns Reticulum) to
`KenAKAFrosty/Prns`, using our local RNode firmware LR1110 driver as the reference
port. Target Prns branch: `main`. License target: dual MIT/Apache-2.0.

Status: de-risking / design pass. Not yet implemented.

## Verdict

Feasible and well-scoped. Prns already ships the entire nRF52840 Hopspot platform
(MCU, embassy executor, nrf-softdevice S140 BLE, embassy-usb, flash, personal-rns
`lora`/`bluetooth-auto`/`usb` features) for the LilyGo T-Echo. T1000-E is the same
MCU family (nRF52840) with a different radio (Semtech LR1110 instead of SX1262) and
no display. The MCU/BLE/USB/flash layer is reusable as-is; the work is the LR1110
radio driver + a T1000-E board variant + a flash-manifest catalog entry.

What Prns does NOT have today: any LR1110 driver, any T1000-E board, and (relevant
to flashing) no Nordic serial-DFU transport.

## Critical finding: there is no `Radio` trait

Prns's embedded LoRa path is **hardwired to a concrete `Sx126x<SPI, BUSY, DIO1,
RST, DLY>` struct**, monomorphized, not generic over a radio trait.

- `LoRaInterfaceInput.radio` and `LoRaInterface.radio` fields are typed
  `Sx126x<...>` (`prns-interfaces/impls/embassy/src/lora.rs:803`, `:815`).
- `LoRaInterface::new` takes a concrete `Sx126x` (`lora.rs:834`).
- `sx126x_config(profile)` (`lora.rs:554`) translates the portable `RadioProfile`
  into `sx126x::RadioConfig` with hardcoded SX1262 op-codes/enums.
- `prns-interfaces/impls/embassy/src/radios/mod.rs` is literally `pub mod sx126x;`
  — one driver, no trait, no dispatch.

So adding LR1110 is a **design decision**, not a drop-in. Two options:

- **A (recommended): introduce a `Radio` trait.** Both `Sx126x` and `Lr1110`
  implement it; `LoRaInterface` becomes generic over `R: Radio`. The config
  translation (`RadioProfile -> chip config`) moves into each driver. This matches
  Prns's CONTRIBUTING rule "give every concept one authoritative home" — a `Radio`
  trait is the missing concept, and `lora.rs` is the hotspot that should be
  extracted from. Blast radius: `lora.rs` field types, `Interface::run` impl, and
  `sx126x_config` relocate into the driver. Needs maintainer buy-in on the
  abstraction shape.
- **B (not recommended): duplicate `lora.rs` as an `lr1110` interface variant.**
  Violates the ownership/DRY rule; likely rejected.

The rest of this doc assumes option A.

## De-facto radio method contract (the surface `Lr1110` must expose)

All async. Source: `prns-interfaces/impls/embassy/src/radios/sx126x.rs`. The
consumer (`LoRaInterface`, `lora.rs`) calls only `init`, `arm_rx`, `read_event`,
`poll_event`, `transmit`, `channel_rssi_dbm` (plus `init` again on channel change).

```rust
// Config / value types (mirror sx126x.rs, adapt enums to LR1110)
enum TcxoVoltage { /* V1_6 .. V3_3  -- ADD V1_8 for LR1110 */ }            // sx126x.rs:70
enum SpreadingFactor { Sf5..=Sf12 }                                       // :84
enum Bandwidth { Bw125, Bw250, Bw500 }                                   // :98
enum CodingRate { Cr4_5..=Cr4_8 }                                        // :107
enum Modulation { Lora { spreading_factor, bandwidth, coding_rate } }    // :115
struct LoraPacket { preamble_symbols: u16, explicit_header: bool, crc_on: bool, invert_iq: bool } // :124
struct RadioConfig { frequency_hz: u32, modulation: Modulation, packet: LoraPacket, sync_word: u16, tx_power_dbm: i8 } // :131
struct BoardConfig { tcxo_voltage: Option<TcxoVoltage>, use_dcdc: bool, rx_boost: bool, dio2_as_rf_switch: bool, external_rx_gain_db: u8 } // :180
enum Error { Spi, Busy, Dio1, Reset, Crc, Timeout, BufferTooSmall }      // :192
struct ReceivedAirFrame { len: usize, phy: PacketPhyStats }              // :203
enum RadioEvent { PreambleDetected, HeaderValid, Frame(ReceivedAirFrame), HeaderError, CrcError, Timeout, Other } // :214

// Driver
struct Lr1110<SPI, BUSY, IRQ, RST, DLY> { /* spi, busy, irq, reset, delay, config, cached freq/mod/pkt/txp, staging */ }
// bounds: SPI: SpiDevice, BUSY: Wait, IRQ: Wait, RST: OutputPin, DLY: DelayNs

impl Lr1110<...> {
    fn new(spi, busy, irq, reset, delay, config: BoardConfig) -> Self;
    async fn init(&mut self, config: RadioConfig) -> Result<(), Error>;          // cold-start + apply channel; leave standby
    async fn transmit(&mut self, payload: &[u8]) -> Result<(), Error>;            // stage RAM, SetTx, wait IRQ high for TxDone
    async fn arm_rx(&mut self) -> Result<(), Error>;                             // SetRx continuous (0xFFFFFF rtc steps)
    async fn read_event(&mut self, buf: &mut [u8]) -> Result<RadioEvent, Error>;  // wait IRQ high, read/clear IRQ, decode
    async fn poll_event(&mut self, buf: &mut [u8]) -> Result<Option<RadioEvent>, Error>; // non-blocking IRQ check (pre-TX race close)
    async fn read_frame(&mut self, buf: &mut [u8]) -> Result<ReceivedAirFrame, Error>; // loop read_event until Frame/Crc/Timeout
    async fn receive(&mut self, buf: &mut [u8]) -> Result<ReceivedAirFrame, Error>;     // arm_rx + read_frame
    async fn channel_rssi_dbm(&mut self) -> Result<i16, Error>;                  // carrier-sense for LBT (RSSI inst, not CAD)
}
```

Note: `sx126x_config` (`lora.rs:554`) currently does `RadioProfile -> sx126x::RadioConfig`.
Under option A this becomes `Lr1110::from_profile(profile) -> RadioConfig` inside
the LR1110 driver (and the SX1262 equivalent moves into the SX1262 driver).

## LR1110 method -> lr11xx command mapping

Our reference: `RNode_Firmware/lr1110.cpp` + Semtech `lr11xx_{radio,system,regmem}.h`
(Clear BSD) + `lr11xx_hal_arduino.cpp` (MIT). The Rust driver uses embassy SPI +
async `Wait` on BUSY/IRQ instead of polled loops + ISR.

| Rust method | lr11xx calls (our C reference) | Notes |
|---|---|---|
| `new` | store pins/config | No SPI activity. |
| `init` (cold start) | `lr11xx_hal_reset` (RST low 1ms, high, 150ms, wait BUSY); poll `lr11xx_system_get_version` until `type == LR1110` (0x01), 2s timeout | `lr1110.cpp:83-105` (preInit), `:130-166` (begin). |
| `init` (apply channel) | `set_reg_mode(DCDC)`; `set_dio_as_rf_switch(rfswitch_cfg)`; `set_tcxo_mode(V1_8, 983 ticks=30ms@32768Hz)`; `calibrate(LF_RC|HF_RC|PLL|ADC|IMG|PLL_TX)`, 5ms; `set_pkt_type(LORA)`; `set_standby(RC)`; `set_lora_sync_word(0x12)`; `set_rf_freq(freq)`; `set_pa_cfg(...)`; `set_tx_params(txp, RAMP_48_US)`; `set_lora_mod_params(sf,bw,cr,ldro)`; `set_lora_pkt_params(preamble,header,plen,crc,iq)`; `cfg_rx_boosted(true)`; `set_dio_irq_params(irq_enable, irq_dio)` | `lr1110.cpp:130-166`. RF switch + TCXO are internal to LR1110 (no MCU GPIO). |
| `transmit` | `regmem_write_buffer8(txbuf, len)`; `set_lora_pkt_params(...plen=len)`; `clear_irq_status(ALL)`; `set_tx(0)`; wait IRQ high; `get_and_clear_irq_status` -> expect `TX_DONE` | `lr1110.cpp:179-197`. TX_DONE polled in C; in Rust, wait IRQ async. |
| `arm_rx` | `clear_irq_status(ALL)`; `set_rx_with_timeout_in_rtc_step(0xFFFFFF)` (continuous) | `lr1110.cpp:390-422`. Explicit header, plen=255 (LR1110 drops packets longer than plen in explicit mode). |
| `read_event` / `poll_event` | wait IRQ high (or non-blocking read); `get_and_clear_irq_status`; decode `RX_DONE`/`CRC_ERROR`/`PREAMBLE_DETECTED`/`HEADER_VALID`/`TIMEOUT` | `lr1110.cpp:346-363` (handleDio0Rise). |
| `read_frame` (on RX_DONE) | `get_rx_buffer_status` -> start,len; `regmem_read_buffer8(buf, start, len)` with 256-byte linear-wrap split; `get_lora_pkt_status` -> rssi/snr | `lr1110.cpp:311-325` (loadPacket), RSSI/SNR `:357-410`. LR1110 buffer read is linear (no hardware wrap, unlike sx126x). |
| `channel_rssi_dbm` | `get_rssi_inst` -> int8 dBm | `lr1110.cpp` currentRssi. LBT carrier-sense; CAD not used by our driver. |
| (sleep) | `set_sleep({0}, 0)` | optional. |

IRQ model unification (important): our C firmware routes only `RX_DONE` to DIO0
and polls TX_DONE. Prns SX126x uses one DIO line for both TX+RX. For LR1110, set
`set_dio_irq_params(irq_enable = TX_DONE|RX_DONE|CRC_ERROR|PREAMBLE_DETECTED|
HEADER_VALID, irq_dio = same)` so DIO0 fires for both; `transmit` and `read_event`
both wait the same IRQ line. This is the cleanest fit for Prns's async event loop.
Pin: our DIO0 (nRF52840 pin 33) maps into the `IRQ` (Prns `DIO1`-slot) `Wait` input.

PA config (LR1110-specific, keep inside the driver): `tx_power_dbm: i8` on
`RadioConfig` -> `set_pa_cfg` + `set_tx_params` via Seeed's `LR11XX_PA_LP_LF_CFG_TABLE`
/ `LR11XX_PA_HP_LF_CFG_TABLE` (LP -17..+15 dBm, HP -9..+22 dBm; LP uses
pa_sel=LP/_supply=VREG, HP uses pa_sel=HP/supply=VBAT; ramp 48us). Tables in
`lr1110.cpp:24-53`, sourced from Seeed `ral_lr11xx_bsp.c`. Port the tables verbatim
into the Rust driver. Prns's `RadioConfig.tx_power_dbm` already carries the i8, so
no API change needed.

RF switch (LR1110-specific, keep inside the driver): LR1110 uses internal
`set_dio_as_rf_switch` with an 8-field `rfswitch_cfg` (enable/standby/rx/tx/tx_hp/
tx_hf/gnss/wifi over RFSW0..RFSW3), no MCU GPIO. This is richer than SX126x's
`BoardConfig.dio2_as_rf_switch: bool`. Do NOT extend `BoardConfig`; bake the
rfswitch_cfg into the LR1110 driver init (it is LR1110/T1000-E radio config, not
MCU config). `BoardConfig.dio2_as_rf_switch` is simply ignored by the LR1110
driver.

## T1000-E embassy-nrf pin/port table

Arduino Adafruit/Seeeduino nRF52 pin numbers from `Boards.h` T1000E block
(`:914-965`) and `lr1110.h:18-22`. Must be translated to embassy-nrf `Peri`
ports via the `Seeed_XIAO_nRF52840` / `tracker_t1000_e_lorawan` variant
`g_APinDescription` table (open item — do not fabricate ports).

| Function | Arduino pin | nRF52840 port (TODO: confirm vs variant) | embassy role |
|---|---|---|---|
| SPI SS / CS | 12 | P?_?? | `Output`, `ExclusiveDevice` CS |
| SPI SCK | 11 | P?_?? | `Spim` sck |
| SPI MOSI | 41 | P?_?? | `Spim` mosi |
| SPI MISO | 40 | P?_?? | `Spim` miso |
| LR1110 BUSY | 7 | P?_?? | `Input` `Wait` (wait_for_high, 100ms deadline) |
| LR1110 DIO0 / IRQ | 33 | P?_?? | `Input` `Wait` (async wait_for_high) — maps to Prns `DIO1`-slot |
| LR1110 NRESET | 42 | P?_?? | `Output` (RST) |
| TCXO enable | -1 | n/a | internal to LR1110 (`set_tcxo_mode`), no MCU GPIO |
| RXEN / RF switch | -1 | n/a | internal to LR1110 (`set_dio_as_rf_switch`), no MCU GPIO |
| LED RX (green) | 24 | — | status (optional) |
| LED TX (red) | 3 | — | status (optional) |
| Button USR1 | 6 | — | input (optional) |
| VBAT ADC (A0) | 2 | — | battery sense (optional) |
| VCC ADC (A1) | 4 | — | |
| Charger ADC (A2) | 5 | — | |

SPI: 4 MHz, MODE0, MSB-first (`lr11xx_hal_arduino.cpp:17`). embassy `Spim` config
M4. Wrap with `embedded_hal_bus::spi::ExclusiveDevice::new(bus, cs, Delay)` (same
as T-Echo `hardware.rs:184`) to satisfy `SpiDevice`.

Pin-conflict check (open item): nRF52840 NFC pins P0_09/P0_10 default to NFC; if
unused they must be reclaimed as GPIO. Confirm none of the T1000-E radio pins
collide with SoftDevice-protected resources (NFC, DCDC pins, LFCLK pins). T-Echo
works with P0_17/19/20/22/23/24/25; T1000-E uses a different pin set (7/11/12/33/
40/41/42) — verify no SoftDevice conflict.

## Config & enum mapping

- `TcxoVoltage`: add `V1_8` (LR1110 uses 1.8V; sx126x enum currently V1_6..V3_3).
  `lr1110.cpp:438`, `system_types.h:284` (`LR11XX_SYSTEM_TCXO_CTRL_1_8V = 0x02`).
- `BoardConfig`: T1000-E uses `tcxo_voltage = Some(V1_8)`, `use_dcdc = true`,
  `rx_boost = true`, `dio2_as_rf_switch = false` (LR1110 has its own internal RF
  switch). `external_rx_gain_db = 0`.
- `RadioConfig.sync_word`: LR1110 sync is a single byte `0x12` (private network),
  not the SX126x two-byte word. Keep LR1110's `set_lora_sync_word(0x12)` inside the
  driver; `RadioConfig.sync_word: u16` can be cast/truncated.
- `Modulation` mapping: LR1110 `set_lora_mod_params(sf, bw, cr, ldro)`. LDRO auto:
  set when `(1<<sf)/(bw_kHz) > 16` (`lr1110.cpp` handleLowDataRate).
- Defaults (T1000-E): SF7, BW125, CR4/5, preamble 18, CRC on, explicit header,
  IQ standard, sync 0x12, payload 255, RX boosted, DCDC, RX continuous.
  (`lr1110.cpp:70-78`, `Config.h:90`).

## Board variant recipe (part B)

Mirror `personal-hopspot/embedded/nrf52840/src/boards/t_echo/`. **T1000-E has no
display** -> drop `epd-waveshare`/`embedded-graphics` deps and the screen render
task; the `render` future in `firmware.rs:604` becomes `pending()` or a no-op.
This is simpler than T-Echo.

Add (under `personal-hopspot/embedded/nrf52840/`):
- `src/boards/t1000e/mod.rs` — re-export `Board`/`Controls`/`Persistence`/`Storage`/
  identity/USB descriptors/`RADIO_PROFILE_PAGES` (cf. `t_echo/mod.rs`).
- `src/boards/t1000e/hardware.rs` — `T1000eBoard`, `T1000eDeferredHardware`, `finish()`
  constructing `Lr1110` + handing to `LoRaInterface` via `ExclusiveDevice<Spim,...>`
  + `Input`/`Output` (cf. `t_echo/hardware.rs`). Pins per table above.
- `src/boards/t1000e/identity.rs` — `bootstrap_ble_identity` +
  `bootstrap_node_identity` + `startup_notice` (cf. `t_echo/identity.rs`).
- `src/boards/t1000e/persistence.rs` — wraps `SharedNorFlash` (cf. `t_echo/persistence.rs`).
- `src/boards/t1000e/storage.rs` — `T1000eStorage` (cf. `t_echo/storage.rs`).
- No `display.rs`/`ssd1681.rs`/`input.rs` (no display).

Edit:
- `Cargo.toml` — add `board-t1000e = []` feature, `[[bin]] name = "t1000e" path =
  "src/bin/t1000e.rs" required-features = ["board-t1000e"]`; drop epd/graphics deps
  for this feature (feature-gate them to `board-t-echo`).
- `src/boards/mod.rs` — `#[cfg(feature = "board-t1000e")] pub(crate) mod t1000e;`
  + selected-board alias.
- `src/bin/t1000e.rs` — `#![no_std]`/`#![no_main]` entry calling
  `personal_hopspot_nrf52840::run(spawner).await` (cf. `main.rs`).
- `memory.x` — T1000-E SoftDevice layout. T-Echo: FLASH 0x27000/0x99000, RAM
  0x2000E000/0x32000. T1000-E uses S140 7.x (same SoftDevice) so the reservation is
  likely near-identical; confirm against T1000-E ldscript (Seeeduino core) and
  adjust RAM origin/size.
- `personal-hopspot/core/src/flash_layout.rs` — add
  `T1000E_RADIO_PROFILE_PAGES: [u32;2]` + journal/arena layout (cf.
  `T_ECHO_RADIO_PROFILE_PAGES` at `:40-42`) + compile-time asserts.
- `personal-rns/src/interface_families/radios.rs` — add
  `pub mod lr1110 { pub use prns_interfaces_embassy::radios::lr1110::{...}; }`.
- `prns-interfaces/impls/embassy/src/radios/mod.rs` — add `pub mod lr1110;`.
- `prns-interfaces/impls/embassy/src/lora.rs` — generalize over `R: Radio`
  (option A): field types, `Interface::run` impl, move `sx126x_config` into the
  SX1262 driver; add the LR1110 equivalent. **Central design change.**

SoftDevice note: our Arduino T1000-E build uses the Seeeduino core + Bluefruit.
The Prns port does NOT — it uses embassy-nrf + the `nrf-softdevice` crate (same as
T-Echo), talking to the S140 binary over SVCs. So the Arduino-core-specific
issues (Seeeduino vs Adafruit, InternalFS NVMC patch, Bluefruit bond_init) do not
apply. Only the SoftDevice binary + memory.x + pin conflicts matter, and T-Echo
already proves S140+embassy works on nRF52840.

## Flash transport (part C) — open question

T1000-E stock bootloader = **Nordic serial DFU** (flash via
`adafruit-nrfutil dfu serial --package <zip>`, 1200-baud touch to enter bootloader;
bootloader-mode USB id `Seeed_Studio_T1000-E_*`). NOT UF2 mass-storage.

Prns `Transport` enum (`prns-flash-manifest/src/catalog.rs`) has only:
- `EspSerial` (Espressif serial bootloader)
- `Uf2MassStorage` (UF2 mass-storage copy)

T-Echo (also nRF52840) uses `Uf2MassStorage` — LilyGo ships a UF2 bootloader. T1000-E
ships serial-DFU. So the T1000-E flash-manifest entry cannot reuse an existing
transport. Options:
- **C1**: Add `Transport::NrfSerialDfu` + flasher support (a Rust implementation of
  adafruit-nrfutil DFU, or shell out to `adafruit-nrfutil`). Bigger flasher work;
  benefits all nRF52 serial-DFU boards.
- **C2**: Document flashing T1000-E manually with `adafruit-nrfutil dfu serial`
  and ship only the firmware artifact (`.zip` DFU package) from the build, leaving
  the Prns web flasher unsupported for T1000-E until C1 lands. Lowest friction for
  the first PR.
- **C3**: Reflash T1000-E with a UF2 bootloader (Adafruit nRF52 UF2) so it can reuse
  `Uf2MassStorage`. Invasive — replaces the device's bootloader; users must DFU the
  UF2 bootloader once first. Not recommended for a contribution.

Recommendation: ship parts A+B + catalog entry with `transport` marked
not-yet-supported (or `C2` manual instructions), and raise `C1` as a follow-up
issue. The firmware is the valuable contribution; the web flasher transport is
orthogonal and should not block the PR.

Catalog entry sketch (to add to `SHIPPING_BOARD_SLUGS` and `boards.json`):
```
slug: "t1000-e"
display_name: "Seeed SenseCAP T1000-E"
silicon: "nRF52840 + Semtech LR1110"
interfaces: ["lora","bluetooth","usb"]
icon: <see prns.dev icon set>
transport: <C1/C2 decision>
expected_chip: None   # nRF52, not Espressif
flash_size: 1024*1024
preparation_profile: <nrf52 serial-dfu profile, new>
provisioning: <see T-Echo provisioning descriptor>
build: <BoardBuild::Uf2 or a new NrfDfu build variant>
```

## License

- Our `lr1110.h`/`lr1110.cpp`/`lr11xx_hal_arduino.{h,cpp}`: MIT (Mark Qvist).
- Semtech `lr11xx_*.{h,c}`: Clear BSD (attribution + disclaimer retention required).
- Prns contribution license: dual MIT/Apache-2.0. MIT and Clear BSD are both
  compatible. The Rust LR1110 driver should be a fresh implementation licensed
  MIT/Apache-2.0 (Prns house style), with the Semtech Clear BSD notice retained in
  a header comment crediting Semtech for the command-set reference. Do not lift
  the C verbatim; re-implement against the lr11xx command set.

## PR scope / effort / risks

- **Part A (LR1110 driver + Radio trait)**: long pole. Trait + generalize `lora.rs`
  + new `lr1110` module + config/profile translation + PA tables + IRQ model.
  Bounded: the lr11xx command set is well-documented and our C driver is a full
  reference. ~1-2k Rust lines. Highest maintainer-review surface (the trait shape).
- **Part B (T1000-E board variant)**: small. Mirror T-Echo, swap pins + radio,
  drop display. Needs the variant pin map (open item) + memory.x + flash_layout
  entries.
- **Part C (flash manifest)**: small for the catalog entry; the transport is the
  open question (C1/C2 above).

Open questions to resolve before/with the PR:
1. Maintainer's preferred abstraction: `Radio` trait (option A) vs another shape.
2. T1000-E Arduino-pin -> nRF52840 port map (from Seeed variant
   `g_APinDescription`) — needed for `hardware.rs`.
3. T1000-E memory.x (SoftDevice S140 flash/RAM reservation) — confirm vs T-Echo.
4. T1000-E LFCLK source (LFRC vs LFXO) for nrf-softdevice — confirm.
5. Pin-conflict check vs SoftDevice-protected NFC/DCDC pins.
6. Flash transport decision (C1/C2).
7. Whether `personal-hopspot-core` screen/node_pages module must be feature-gated
   for no-display boards (T1000-E), or already optional.