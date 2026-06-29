# Personal Hopspot — iOS

The iOS face of Personal Hopspot. The shared `personal-hopspot-ui` renderer
(generic over `embedded_graphics::DrawTarget<Color = BinaryColor>`) draws the
identical 64x128 screen here that it draws on the Heltec V4 OLED, the Linux debug
window, and the Android app. This face adds the platform adapters iOS needs:

- a `DrawTarget` backed by a flat RGBA framebuffer (`rust/src/framebuffer.rs`)
- a single-button input source: every tap is a `ShortPress`, every hold a
  `LongPress` (`rust/src/face.rs` + the `hopspot_post_input` entry point)
- a real `personal-rns` runtime with WiFi/LAN, Bonjour discovery, BLE Auto, and
  USB Auto over a usbmux-forwarded byte stream

`rust/` is a C-ABI `staticlib` linked straight into the app binary (iOS has no
JNI; the seam is `extern "C"` instead of Android's JNI exports). The
Swift/SwiftUI shell that hosts it lives in `app/`.

The iOS USB Auto lane is intentionally one-directional for now: the iPad app acts
as the USB Auto device and the Mac/desktop Hopspot acts as the host. The transport
rides `iproxy`/usbmux over the physical USB cable, so the app gets an ordinary
TCP listener while the desktop host still sees one USB Auto byte pipe.

## Native ABI — `rust/include/hopspot.h`

```c
HopspotFace *hopspot_init(void);
void         hopspot_free(HopspotFace *handle);
int32_t      hopspot_post_input(HopspotFace *handle, int32_t code); // code: 0 tap, 1 hold; returns 0 none, 1 announce
void         hopspot_render(HopspotFace *handle, uint8_t *ptr, size_t len); // fills width*height*4 RGBA bytes
uint32_t     hopspot_panel_width(void);
uint32_t     hopspot_panel_height(void);
```

The render path is pull-model: `HopspotBridge` owns a heap RGBA buffer, Rust draws
the current `UiState` into it each frame, and SwiftUI blits it nearest-neighbor
(`Image(...).interpolation(.none)`) into a `CGImage`. The panel is 64x128; bytes
are `[R, G, B, A]` per pixel. Panel dimensions come from the two `hopspot_panel_*`
functions so Rust stays the single source of truth.

## One-time toolchain setup

Building for the simulator needs the full Xcode (not just Command Line Tools):

```sh
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer   # point at full Xcode
sudo xcodebuild -license accept                                    # accept the license
xcodebuild -downloadPlatform iOS                                   # install the iOS Simulator runtime (multi-GB)
rustup target add aarch64-apple-ios-sim aarch64-apple-ios          # Rust iOS targets
```

## Build the Rust static lib (standalone)

From `rust/`:

```sh
cargo build --release --target aarch64-apple-ios-sim
```

Produces `rust/target/aarch64-apple-ios-sim/release/libpersonal_hopspot_ios.a`.
The Xcode project also runs this automatically as a "Build Rust static library"
build-phase script (`rust/build-rust.sh`), which picks the cargo triple from the
active `PLATFORM_NAME`/`ARCHS`, so you usually don't run it by hand.

## Build, install, and launch on the simulator

This is an Apple-Silicon, simulator-only proving ground; build for a **concrete
arm64 iPad simulator** (a `generic/platform=iOS Simulator` build is universal and
would also demand an `x86_64` slice we don't build):

```sh
SIMID=$(xcrun simctl create "Hopspot-iPad" \
  com.apple.CoreSimulator.SimDeviceType.iPad-Pro-11-inch-M4-8GB \
  com.apple.CoreSimulator.SimRuntime.iOS-26-5)
xcrun simctl boot "$SIMID"
open -a Simulator

cd app
xcodebuild -project PersonalHopspot.xcodeproj -scheme PersonalHopspot \
  -configuration Debug -destination "id=$SIMID" -derivedDataPath build build
xcrun simctl install "$SIMID" build/Build/Products/Debug-iphonesimulator/PersonalHopspot.app
xcrun simctl launch  "$SIMID" com.personal.hopspot
```

Or just open `app/PersonalHopspot.xcodeproj` in Xcode, pick an iPad simulator, and
press Run.

To grab a screenshot of the running screen:

```sh
xcrun simctl io "$SIMID" screenshot hopspot.png
```

## USB Auto over an attached iPad

With the physical iPad connected, trusted, and visible to Xcode:

```sh
cd app
xcodebuild -project PersonalHopspot.xcodeproj -scheme PersonalHopspot \
  -configuration Debug -destination "id=00008027-000E05943E53802E" \
  -derivedDataPath build build
xcrun devicectl device install app \
  --device 00008027-000E05943E53802E \
  build/Build/Products/Debug-iphoneos/PersonalHopspot.app
xcrun devicectl device process launch \
  --device 00008027-000E05943E53802E com.personal.hopspot
```

Start desktop Hopspot normally from the app crate. On macOS, the desktop USB
host discovers USB-attached iOS devices, starts the `iproxy`/usbmux forwarder,
uses that local byte pipe as a USB Auto target, and tears the helper process
down when the USB stream closes:

```sh
cd ../app
cargo desktop
```

The manual socket path remains available as a diagnostic override when you want
to provide your own forwarding process:

```sh
HOPSPOT_USBMUX_TARGET=127.0.0.1:42700 cargo desktop
```

## Host-side checks (no Xcode or simulator required)

From `rust/`:

```sh
cargo test
```
