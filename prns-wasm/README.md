# prns-wasm

The cooperative WebAssembly backend for the unified `personal-rns` JavaScript
package. Applications normally import `personal-rns` or
`personal-rns/browser`; the browser entry loads the bundled WebAssembly module
and presents the same bounded, casework-shaped event contract as the native
Node.js and Bun entry.

The browser-facing transport helpers live under `prns.interfaces`: WebUSB and
Bluetooth LE talk to nearby devices, while `prns.interfaces.webSocket.connect(url)`
opens a browser WebSocket client to a local or public Prns WebSocket endpoint.
Browser WebSocket clients detect raw-packet, HDLC, or KISS framing from inbound
traffic and fall back to raw-packet framing when a silent peer first needs
outbound traffic. Callers can select a fixed framing when the endpoint contract
is already known. Stable `BrowserRendezvous` interfaces remain fixed to raw
packet framing.

Fallible host operations resolve semantic tagged outcomes. They do not reject
for expected conditions such as cancellation, unavailable browser APIs,
duplicate connections, transport failure, stream ownership conflicts, or
runtime rejection.

```ts
import { Prns, match } from "personal-rns/browser";

const created = await Prns.create({});
match(created, {
  Ready: (node) => {
    const claim = node.claimDiagnostics();
    if (claim.tag === "AlreadyClaimed") {
      reportConsumerConflict(claim.data.lane);
      return;
    }
    consumeDiagnostics(claim.data);
  },
  WasmLoadFailed: handleWasmLoadFailure,
  ContractMismatch: handleContractMismatch,
  IdentityStoreFailed: handleIdentityStoreFailure,
  StoredIdentityInvalid: handleStoredIdentityInvalid,
  HostApiUnavailable: handleUnavailableHostApi,
  EntropySourceFailed: handleEntropyFailure,
  InsufficientEntropy: handleInsufficientEntropy,
  RuntimeRejected: handleRuntimeRejection,
});
```

The package exports its zero-dependency `Tag`, `match`, `match_into`, and
`from` primitives for exhaustive handling and application-defined tagged
unions. Synchronous branded-value constructors still throw
`PrnsValidationError` when the caller violates their immediate input contract.

Resource sending has the same settlement contract as the native backend.
`sendResource` accepts a `Uint8Array`; `sendResourceBlob` incrementally reads a
browser `Blob`. Both use the Rust resource planner and wire engine compiled into
WebAssembly. Automatic bzip2 candidate generation runs off the main thread,
keeps at most two segments in flight, and falls back to the uncompressed segment
when a Worker is unavailable.

## Browser Transport Playground

The documentation playground is a plain TypeScript browser application under
`examples/browser-playground`. It runs a WebAssembly node, keeps Auto Wi-Fi and
USB Auto behind explicit clicks, registers an LXMF delivery destination, and
displays live gateway, interface, single-packet, announce, and outcome activity.
It is deliberately a transport demonstration rather than a messaging client.

Build it and stage its static assets into the documentation site:

```sh
./tools/prns build wasm-docs stage
```

The staged playground uses the size-optimized release WebAssembly profile. `build:browser` remains the
faster debug build for local smoke work.

Raw runtime resource events name byte quantities explicitly, including
`uncompressedDataBytes` and `totalSizeBytes`.

Serve the documentation public directory from the repo root:

```sh
python3 -m http.server 8878 --bind 127.0.0.1 --directory docs/website/public
```

Open:

```text
http://127.0.0.1:8878/browser-node-playground-console/
```

The lower-level browser smoke bundle remains available through
`npm --prefix prns-wasm run build:browser` for development checks.

## Linux WebUSB Setup

Linux desktops usually need a udev rule before Chrome can open the Prns USB
Auto vendor interface. Without it, Chrome can show the device picker but
`device.open()` fails with `SecurityError: Access denied`.

Install the narrow Prns WebUSB rule:

```sh
./tools/prns device webusb install
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
