# Personal Hopspot — Android

The Android face of Personal Hopspot. The shared `personal-hopspot-core` renderer
(generic over `embedded_graphics::DrawTarget<Color = BinaryColor>`) draws the
identical 64x128 screen here that it draws on the Heltec V4 OLED and the Linux debug
window. This crate adds the platform adapters Android needs:

- a `DrawTarget` backed by a flat RGBA framebuffer (`rust/src/framebuffer.rs`)
- a single-button input source: every tap is a `ShortPress`, every hold a
  `LongPress` (`rust/src/face.rs` + the `nativePostInput` entry point)
- Android-hosted USB Auto (`app/src/main/java/org/personal/hopspot/UsbLink.kt`)
- Wi-Fi Auto/mDNS and Bluetooth LE Auto bridges into the shared Rust engine

`rust/` is the JNI `cdylib`. The Kotlin app shell in `app/` hosts it with a
plain Android `View` so the same APK can run on old Android devices as well as
modern phones.

## Native ABI — `org.personal.hopspot.NativeBridge`

```
nativeInit(storageDir) -> long handle
nativePostInput(handle, code) -> int action   // code: 0 = tap, 1 = hold; action: 0 = none, 1 = announce, 2 = copy shared-instance config
nativeRender(handle, directByteBuffer)         // fills PANEL_WIDTH * PANEL_HEIGHT * 4 RGBA bytes
nativeFree(handle)
```

The render path is pull-model and zero-copy: Kotlin owns a direct `ByteBuffer`,
Rust draws the current `UiState` into it, Kotlin blits it nearest-neighbor into an
`ARGB_8888` `Bitmap`. The panel is 64x128; bytes are `[R, G, B, A]` per pixel.

## USB role model

Android Hopspot supports both Android-side USB roles where the OS permits them.
A phone with OTG support can act as the host for an attached embedded Hopspot or
RNode-style device. A desktop host can also negotiate Android Open Accessory
(AOA), after which the phone exposes an app-owned accessory stream and the same
Prns USB Auto wire runs over that stream.

`UsbLink.kt` deliberately keeps the consumer-facing idea as "USB Auto" while the
platform adapter chooses the Android transport:

- direct vendor bulk endpoints for the unified Prns USB Auto VID/PID
  (`1209:0001`)
- CDC ACM serial as a fallback for older development-board and legacy firmware
  shapes
- Android Open Accessory streams for the phone-as-device direction

Stock Android apps are not arbitrary USB gadgets: a Pixel can expose system USB
functions such as `adb`, MTP, or tethering, but an ordinary app cannot simply
publish the raw Prns `1209:0001` gadget endpoint. The phone-as-device direction
therefore uses a distinct physical lane: Android Open Accessory negotiation by
the external host, followed by the same Prns USB Auto wire over the accessory
bulk stream. That gives phones the "take either role when possible" behavior
without pretending every platform exposes the same low-level USB function.

## Toolchain (installed and verified on this machine)

Installed via Homebrew (no sudo) + sdkmanager; env is persisted in `~/.zshrc`:

- JDK 17 (`openjdk@17`) — `JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home`
- Android SDK at `ANDROID_HOME=/opt/homebrew/share/android-commandlinetools` (platform-tools, platform android-34, build-tools 34.0.0)
- NDK r27c — `ANDROID_NDK_HOME=$ANDROID_HOME/ndk/27.2.12479018`
- Rust targets `aarch64-linux-android` and `armv7-linux-androideabi` + `cargo-ndk`
- Gradle 8.7 via the committed wrapper (`./gradlew`); the system Gradle is 9.x, too new for AGP 8.5.2, so always use the wrapper

## Build the `.so`

From `rust/`:

```
cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build --release
cargo ndk -t armeabi-v7a -P 21 -o ../app/src/main/jniLibs build --release
```

Produces:

- `app/src/main/jniLibs/arm64-v8a/libpersonal_hopspot_android.so`
- `app/src/main/jniLibs/armeabi-v7a/libpersonal_hopspot_android.so`

The `armeabi-v7a` build uses API 21 because NDK r27 no longer ships API 19
native platform bits. The Rust bridge provides the one missing pre-21 loader
symbol needed by the MF97B Android 4.4.4 projector path.

## Build, install, and launch on a device

From this directory (`personal-hopspot/mobile/android`), with the phone plugged in and
USB debugging authorized (`adb devices` should list it):

```
./gradlew installDebug
adb shell am start -n org.personal.hopspot/.MainActivity
```

`installDebug` packages whatever `.so` is in `jniLibs/`, so rebuild the `.so`
first whenever the Rust changes. To install the prebuilt APK directly:

```
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

## Host-side checks (no NDK or device required)

From `rust/`:

```
cargo test
```
