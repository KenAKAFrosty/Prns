# personal-rns-wasm

Browser and JavaScript-host bindings for Prns. The TypeScript layer is the
consumer API; the runtime still lives in the shared Rust core.

## Browser Smoke

Build the TypeScript smoke bundle:

```sh
npm --prefix personal-rns-wasm run build:smoke
```

Serve the package directory from the repo root:

```sh
python3 -m http.server 8878 --bind 127.0.0.1 --directory personal-rns-wasm
```

Open:

```text
http://127.0.0.1:8878/smoke/
```

The smoke page verifies the in-browser runtime path, then can open a Hopspot
USB Auto device through WebUSB. A successful USB run shows a confirmed peer,
an announce event, and a snapshot with one active interface.

## Linux WebUSB Setup

Linux desktops usually need a udev rule before Chrome can open the Hopspot USB
Auto vendor interface. Without it, Chrome can show the device picker but
`device.open()` fails with `SecurityError: Access denied`.

Install the narrow Prns WebUSB rule:

```sh
./scripts/install-prns-webusb-udev.sh
```

Then unplug and replug the device, restart Chrome if it had already failed, and
retry the smoke page.

The rule grants the active logged-in seat access only to the Prns WebUSB VID/PID
currently used by Hopspot USB Auto devices:

```udev
SUBSYSTEM=="usb", ATTR{idVendor}=="1209", ATTR{idProduct}=="0001", MODE="0660", TAG+="uaccess"
```

