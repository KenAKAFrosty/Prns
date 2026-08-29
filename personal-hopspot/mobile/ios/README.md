# Personal Hopspot — iOS

The iOS face of Personal Hopspot. `personal-hopspot-core` renders the same
canonical 64×128 monochrome frame used by embedded, desktop, and Android
Hopspots, then the iOS adapter expands that frame into the caller-owned RGBA
surface. This face adds the other platform adapters iOS needs:

- canonical-frame expansion into a caller-owned RGBA buffer
  (`rust/src/face.rs`)
- a single-button input source: every tap is a `ShortPress`, every hold a
  `LongPress` (`rust/src/face.rs` + the `hopspot_post_input` entry point)
- a real `personal-rns` runtime with Wi-Fi/LAN, Bonjour discovery, Bluetooth LE Auto, and
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
int32_t      hopspot_start_engine(const char *storage_directory_utf8);
int32_t      hopspot_stop_engine(void);
int32_t      hopspot_engine_state(void);
int32_t      hopspot_engine_last_failure(void);
HopspotFace *hopspot_init(void);
void         hopspot_free(HopspotFace *handle);
int32_t      hopspot_post_input(HopspotFace *handle, int32_t code); // code: 0 tap, 1 hold; returns 0 none, 1 announce
void         hopspot_render(HopspotFace *handle, uint8_t *ptr, size_t len); // fills width*height*4 RGBA bytes
uint32_t     hopspot_panel_width(void);
uint32_t     hopspot_panel_height(void);
size_t       hopspot_rgba_bytes(void);
uint32_t     hopspot_render_interval_millis(void);
```

`EngineController` owns the restartable native lifecycle and durable Application
Support path. `HopspotBridge` owns only a renderer and heap RGBA buffer. Rust
draws the current `UiState` at the exported cadence, and SwiftUI blits it
nearest-neighbor (`Image(...).interpolation(.none)`) into a `CGImage`. Dimensions,
allocation size, and cadence all come from Rust.

Rust keeps runtime persistence in the private `prns/` directory below
`Application Support/PersonalHopspot`. Startup verifies that directory is
writable, restores the monotonic timebase, verified routes, destination
identities, and tunnels, then publishes the engine as running. Accepted
announces and route removals trigger a debounced snapshot; a quiet five-minute
checkpoint and a final pre-teardown shutdown flush cover other changes.
Malformed stored rows are refused and reported without preventing the valid
remainder from starting.

The opaque 1024×1024 app icon is rendered deterministically from the repository
favicon:

```sh
rsvg-convert --background-color '#0b0e13' --width 1024 --height 1024 \
  --keep-aspect-ratio --format png \
  --output app/PersonalHopspot/Assets.xcassets/AppIcon.appiconset/AppIcon.png \
  ../../../docs/website/public/assets/favicon.svg
```

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

The registered simulator gate selects one concrete available iPhone or iPad and
supports both arm64 and x86_64 macOS hosts:

```sh
python3 validation/run.py run --suite ios-simulator
```

For a manual arm64 build:

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
DEVICE_ID=replace-with-device-udid
xcodebuild -project PersonalHopspot.xcodeproj -scheme PersonalHopspot \
  -configuration Debug -destination "id=$DEVICE_ID" \
  -derivedDataPath build build
xcrun devicectl device install app \
  --device "$DEVICE_ID" \
  build/Build/Products/Debug-iphoneos/PersonalHopspot.app
xcrun devicectl device process launch \
  --device "$DEVICE_ID" com.personal.hopspot
```

Start desktop Hopspot normally from the repository root. On macOS, the desktop USB
host discovers USB-attached iOS devices, starts the `iproxy`/usbmux forwarder,
uses that local byte pipe as a USB Auto target, and tears the helper process
down when the USB stream closes:

```sh
cargo run --manifest-path personal-hopspot/desktop/Cargo.toml --locked
```

The manual socket path remains available as a diagnostic override when you want
to provide your own forwarding process:

```sh
PRNS_USB_AUTO_USBMUX_TARGET=127.0.0.1:42700 \
  cargo run --manifest-path personal-hopspot/desktop/Cargo.toml --locked
```

`HOPSPOT_USBMUX_TARGET` remains a lower-precedence compatibility alias.

## Host-side checks (no Xcode or simulator required)

From `rust/`:

```sh
cargo test
```

The iOS app is publicly available as a pre-1.0 **Shipping** surface. The
automated simulator gate covers its baseline build and launch behavior, and a
current-commit iPad smoke covers the immediate device, persistence, and
Bluetooth path.

Formal production qualification remains open until the complete physical
iPhone and iPad matrix in `validation/platforms/ios-production-hardware.md` has
passed and its separate record-only evidence commit is present. Shipping does
not imply App Store or TestFlight availability, continuous background
execution, completed iPhone evidence, or completion of that qualification.
