# prns-wasm

Browser and JavaScript-host bindings for Prns. The TypeScript layer is the
consumer API; the runtime still lives in the shared Rust core.

The browser-facing transport helpers live under `prns.interfaces`: WebUSB and
Bluetooth talk to nearby devices, while `prns.interfaces.webSocket.connect(url)`
opens a browser WebSocket client to a local or public Prns WebSocket endpoint.
Each binary WebSocket message carries one Prns wire frame.

Fallible host operations resolve semantic tagged outcomes. They do not reject
for expected conditions such as cancellation, unavailable browser APIs,
duplicate connections, transport failure, or runtime rejection.

```ts
const created = await Prns.create({ wasm });
if (created.tag !== "Ready") {
  handleCreationFailure(created);
  return;
}

const connected = await created.data.interfaces.webSocket.connect(url);
if (connected.tag !== "Connected") {
  handleConnectionFailure(connected);
  return;
}

const session = connected.data;
if (session.status.tag === "Failed") {
  handleSessionFailure(session.status.data);
}
```

The package exports its zero-dependency `Tag`, `match`, `match_into`, and
`from` primitives for exhaustive handling and application-defined tagged
unions. Synchronous branded-value constructors still throw
`PrnsValidationError` when the caller violates their immediate input contract.

## Browser Node Playground Smoke

Build the WASM package and TypeScript smoke bundle:

```sh
npm --prefix prns-wasm run build:browser
```

Serve the package directory from the repo root:

```sh
python3 -m http.server 8878 --bind 127.0.0.1 --directory prns-wasm
```

Open:

```text
http://127.0.0.1:8878/smoke/
```

The smoke page is a small browser node playground. It verifies the in-browser
runtime path, opens a USB Auto device through WebUSB, shows live
interface snapshots, and logs announces and command events in readable form. A
successful USB run shows a confirmed peer, an announce event, and a snapshot
with one active interface.

## Linux WebUSB Setup

Linux desktops usually need a udev rule before Chrome can open the Prns USB
Auto vendor interface. Without it, Chrome can show the device picker but
`device.open()` fails with `SecurityError: Access denied`.

Install the narrow Prns WebUSB rule:

```sh
./scripts/install-prns-webusb-udev.sh
```

Then unplug and replug the device, restart Chrome if it had already failed, and
retry the smoke page.

Snap Chromium has an additional sandbox. If WebUSB still fails there, either use
a non-Snap Chrome/Chromium build or grant the snap raw USB access:

```sh
sudo snap connect chromium:raw-usb
```

The rule grants the active logged-in seat access only to the Prns WebUSB VID/PID
currently used by Prns USB Auto devices:

```udev
SUBSYSTEM=="usb", ATTR{idVendor}=="1209", ATTR{idProduct}=="0001", MODE="0660", TAG+="uaccess"
```
