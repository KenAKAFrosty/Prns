# T1000-E firmware recovery

Hopspot keeps the T1000-E stock bootloader and SoftDevice intact. Recovery
entry requests a reset into that bootloader; it does not erase settings or
install another application.

The following shortcuts require a Hopspot build containing recovery entry.
Older installed builds still need the vendor sequence described below. The
startup shortcut also needs the application to reach hardware initialization;
it cannot recover a damaged bootloader or an application that never starts.

## From the flasher

Open the T1000-E page of the Hopspot flasher in current desktop Chrome or Edge,
connect the tracker, and select **Recovery → Restart into recovery**.
Choose the Personal Hopspot device in the USB picker. This action works without
preparing a Hopspot release or downloading firmware.

Chrome requires a button click and approval in its device picker. After approval,
the software request can enter recovery without pressing the tracker button or
reconnecting its cable. The flasher provides recovery entry and the Meshtastic
instructions; it does not install Meshtastic itself.

The browser reports a request acknowledgement. Confirm that the `T1000-E`
drive actually appears and that `INFO_UF2.TXT` identifies the T1000-E before
copying firmware. If the request is rejected, use manual recovery; older
firmware does not implement this request. Close other apps connected to the
tracker if the browser cannot claim its USB interface.

## With the button

Leave the USB end of the cable connected to the computer. Remove the magnetic
connector, hold the upper button near the lanyard, and attach the connector
once. Keep holding through the hardware reset and startup until the recovery
drive appears; allow at least six seconds before releasing.

Hopspot checks the active-high button on P0.06 immediately after HAL
initialization. If USB power is present and the button remains held for two
seconds, it enters UF2 recovery before initializing persistent storage,
identity, radio, or the application's USB interface. Normal startup is not
delayed when the button is released. This startup shortcut is implemented but
has not yet been verified with a physical held-button test.

For older firmware or an unresponsive application, hold the same button while
rapidly attaching, removing, and reattaching the magnetic connector. Keep the
computer end connected. Seeed notes that this may require several attempts.
The single-connection hard reset and the rapid double-connection bootloader
sequence are different. A green light alone does not establish that a drive
mounted. See the [vendor instructions](https://wiki.seeedstudio.com/sensecap_t1000_e/).

## Return to Meshtastic

Follow the [Meshtastic nRF52 erase and install guide](https://meshtastic.org/docs/getting-started/flashing-firmware/nrf52/nrf52-erase/).
Use its erase utility matching the SoftDevice in `INFO_UF2.TXT`, then install
the official **T1000-E** application UF2. Erase removes device settings. Do not
substitute a UF2 for a different board. The Hopspot flasher links this guide;
third-party firmware is obtained from its own publisher.

## Local developer flasher

Build and serve the real flasher with a locally signed T1000-E candidate:

```console
./tools/prns run device.hopspot.dev-flasher.serve -- t1000-e --port 8765
```

Open `http://127.0.0.1:8765/flash/t1000-e/` in desktop Chrome. The page marks the
candidate as local developer firmware and verifies its signatures and artifact
hashes before device access.

For the first installation from the tested Meshtastic application, enter the
stock serial DFU bootloader first. That application enumerates as USB
`239a:8029`; the flasher's stock serial DFU entry expects `2886:0057` and does
not issue Meshtastic admin messages. Meshtastic's `enterDFUMode` admin request
over serial was verified to enter this stock bootloader without physical button
presses or cable reconnections. Select the **Seeed/Meshtastic firmware or
bootloader** entry path after the stock bootloader appears, prepare the
candidate, and choose **Connect and update tracker**.

After installing Hopspot with recovery support, **Restart into recovery** on the
same page provides the direct software path back to the stock UF2 drive.

## USB contract

The application identifies itself as USB `1209:0001`, manufacturer
`Stay Personal`, product `Personal Hopspot (T1000-E)`, serial
`PERSONAL-RNS-T1000E-HOP`, with its sole WebUSB interface numbered zero.

Both requests are vendor, device-recipient, OUT control transfers with
`wValue=0x5052`, `wIndex=0x4e53`, and no data stage:

| Request | T1000-E reset mode | Purpose |
| --- | --- | --- |
| `0x50` | GPREGRET `0x4e` | Serial DFU used by the existing Hopspot updater |
| `0x55` | GPREGRET `0x57` | Serial DFU plus the stock UF2 recovery drive |

The canonical command constants belong to `prns-core` USB Auto. The firmware
handler rejects malformed signatures and unsupported modes. The browser checks
the exact application identity and interface before sending the request, closes
the device afterward, and excludes concurrent installs. Entering recovery
discards any prepared install and invalidates pending preparation.

## Physical evidence

On 2026-10-01, a T1000-E running Hopspot 0.3.7 at source commit
`bee0d7000a6235696cd714fb1bb4b6b17f74cf9c` was recovered using the vendor's
rapid connector sequence. Its stock bootloader reported
`0.9.1-5-g488711a`, board ID `nRF52840-T1000-E-v1`, and S140 `7.3.0`.
After the matching Meshtastic erase utility and official application UF2 were
installed, a Meshtastic serial protocol query confirmed firmware
`2.7.26.54e0d8d` and hardware model `TRACKER_T1000_E`.

### Software recovery round trip

Later on 2026-10-01, the same tracker completed the following round trip without
a physical button press, cable reconnection, or requested user intervention:

1. Meshtastic's `enterDFUMode` admin request entered the stock UF2 bootloader.
2. The unreleased Hopspot developer UF2 from source commit
   `fa5f7546b012e3bf0d8c3a3755750d1b1dc4b935` was installed. Its SHA-256 was
   `6f1aef3bd0df1ae17524bcda1d6e3022d214a53c7005de73b9e553dba97b4991`.
   The application enumerated with the exact Hopspot USB identity above.
3. The production `handOffToUf2` JavaScript, bundled from that source and called
   from a localhost test page in desktop Google Chrome, used real
   `navigator.usb`. Native UI automation selected the Hopspot device and approved
   Chrome's picker. The function returned `status: "requested"`.
4. The stock `T1000-E` drive and USB `2886:0057` appeared at the same physical USB
   location. `INFO_UF2.TXT` still identified the T1000-E and S140 `7.3.0`.
   All 2,030 application blocks in its `CURRENT.UF2` readback matched the
   installed developer UF2 exactly.
5. The matching official Meshtastic erase utility enumerated, its serial console
   was opened, and it returned to the stock bootloader. This run did not capture
   its formatting completion text, so it does not independently qualify erasure.
6. The official T1000-E Meshtastic UF2 was installed. A serial protocol query
   confirmed `2.7.26.54e0d8d`, `TRACKER_T1000_E`, and the same node ID and name.
   Local and module configuration messages matched their pretest values. The
   bootloader drive was no longer present.

This qualifies the new software recovery request on the attached tracker and
stock bootloader. It does not qualify the held-button startup shortcut. The
physical browser check exercised the production JavaScript engine through a
local test page; it was not a deployment of the public flasher site. Automated
full-page browser tests use simulated USB devices. The initial headless and
isolated browser harnesses could not approve the native WebUSB picker; using
desktop Chrome with native UI automation completed the hardware check.

## Verification on macOS, 2026-10-01

- `./tools/prns run build.hopspot.t1000e`: recovery UF2 built at application
  base `0x27000`, nRF52840 family `0xADA52840`.
- `cargo test --locked -p prns-flash-manifest`: 79 tests passed.
- `cargo test --locked --manifest-path prns-interfaces/impls/embassy/Cargo.toml --features usb usb_auto::device`:
  five tests passed, including both commands and malformed transfers.
- `cargo test --locked --manifest-path docs/website/Cargo.toml`: 51 tests passed.
- `npm run test:flasher` in `docs/website`: 82 tests passed, including recovery
  without preparation, invalidation, concurrency, cancellation, and failures.
- `npm run test:browser` in `docs/website`: all 36 Chromium tests passed on
  the integrated upstream static website. This includes the recovery UI,
  production Nordic bridge, existing guided installs, and static route
  hydration. Recovery UI accessibility was checked with axe. Before
  integration, four focused tests also passed. The initial browser invocation
  could not launch because its pinned Chromium was absent; subsequent runs
  used that pinned browser after installing it into temporary storage.
- Clippy with warnings denied passed for the manifest, website, USB handler,
  and the T1000-E and T096 target configurations. The shared SoftDevice reset
  callback also passed target Clippy for MeshPocket with
  `mesh-pocket-battery-5000`, Muzi Base Duo with `softdevice-s140-v6`, and
  RAK4631. The first MeshPocket invocation omitted its required battery
  feature and was corrected before compiling that target. Touched Rust files
  were formatted; documentation links and `git diff --check` were clean.

The build and automated checks above preceded the first physical round trip,
which ended with the tracker restored to Meshtastic. Software recovery passed
physical qualification; the held-button route still needs physical
qualification. The new firmware and site changes have not been published.

### Packaged developer build failure

The first local developer flasher built a different application from the
physically qualified UF2. After its serial DFU installation on 2026-10-01,
Hopspot did not enumerate on USB and the recovery picker was empty. The tracker
returned to the stock bootloader after the user followed a held-button recovery
prompt. Its readback matched all 484,184 installed application bytes at
`0x27000`, SHA-256
`b9c68540040d65369215c8df3cd0d5462bc1589910aae954a5069284b887c057`.
Reinstalling the earlier qualified UF2 restored the exact Hopspot USB identity
without another physical action. The failing local candidate was withdrawn.

The packaged build had applied additional Thumb compiler settings:
`--icf=all`, machine outlining, and the compact SHA-2 backend. The developer UF2
recipe did not apply them. Nordic serial DFU builds now use the explicit
`thumbv7em-serial-dfu-rust-lld` adapter with baseline compiler settings, matching
the developer UF2 recipe. Resource reports select that same adapter so their
compiler evidence describes the transferred image. The failure has not been
traced to one compiler setting; the developer version metadata also differed.
A replacement candidate must pass both installation and recovery through the
full flasher page before handoff; the earlier UF2 result alone does not qualify
a rebuilt DFU image.

### Corrected full-page installation and recovery

The replacement local developer flasher from source commit
`1f08f2ee972c5002b6367805bd3c6f47a928dd4e` passed a physical check in desktop
Google Chrome on 2026-10-01. Its version was
`0.3.7-dev.clean.37d7e339a9dae1ab34d85137b378d445519831daaec86a8d556adbc1e862128a`.
This check used the real T1000-E flasher page, its signature verification,
production Nordic DFU engine, and native Web Serial and WebUSB pickers:

1. **Prepare and verify release** verified 519,718 bytes: a 519,704-byte
   application and its 14-byte init packet.
2. **Connect and update tracker**, using the stock bootloader entry path,
   completed serial DFU. The application then enumerated with the exact Hopspot
   USB identity above. The application's SHA-256 was
   `7792896982d4fd8b43c89149e14b77e517261b51ad0a0ad62863c9ae909b12a4`.
3. **Enter recovery mode** on the same page found that installed Hopspot,
   acknowledged the request, and exposed the stock `T1000-E` drive and USB
   `2886:0057` at the same physical USB location.
4. `INFO_UF2.TXT` reported the same T1000-E board ID, bootloader
   `0.9.1-5-g488711a`, and S140 `7.3.0`. All 519,704 application bytes in
   `CURRENT.UF2` matched the browser's installed application at `0x27000`.

Native UI automation selected and approved both browser pickers. The corrected
installation and recovery required no physical button presses, cable
reconnections, or user intervention. The user's earlier physical recovery of
the failed candidate occurred before this check. The check ended in the stock
UF2 bootloader with the corrected Hopspot application retained; it did not
install Meshtastic again or qualify the held-button shortcut.

For this correction, `cargo test --locked -p personal-hopspot-builder -p
personal-hopspot-resources --quiet` passed 44 builder and 97 resource tests.
Clippy with warnings denied passed for both packages and all targets, and
`npm run test:flasher` passed all 82 tests. The local developer site remains
separate from a published release.
