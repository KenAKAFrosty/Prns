# Browser transport playground

This is the quick guide for the static browser playground published with the Prns
documentation. It is intentionally ordinary TypeScript, HTML, and CSS. The
page owns its WebAssembly node exactly as a browser application would. 

The page doesn't use React, Solid, Dioxus, or any other web framework. The reason is to avoid making you have to learn a new framework if you're not familiar with the one we happened to choose, and to keep the example as universally-readable as possible.

The playground keeps Auto Wi-Fi, direct WebSocket, Bluetooth LE, and USB Auto connections behind explicit actions, registers an LXMF delivery destination named `Prns Browser Playground`, and exposes engine snapshots, single-packet payloads, and tagged outcomes for inspection. Web Bluetooth acts as a central and connects to an advertising native or embedded Prns node through the shared native GATT service.

Use the Bluetooth control from a browser that exposes Web Bluetooth over a
secure context. The click opens the browser's device chooser; select a nearby
native or embedded Prns node advertising Bluetooth Auto. The resulting
interface is bidirectional even though the browser itself does not advertise.
On Linux Chromium, the playground explains how to enable the browser's
experimental Web Bluetooth switch when the API is disabled by default.

Build and stage the page into the documentation site's public assets:

```sh
./tools/prns build wasm-docs stage
```

Serve the documentation public directory from the repository root:

```sh
python3 -m http.server 8878 --bind 127.0.0.1 --directory docs/website/public
```

Then open:

```text
http://127.0.0.1:8878/browser-node-playground-console/
```
