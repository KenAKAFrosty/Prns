# Browser transport playground

This is the source for the static browser playground published with the Prns
documentation. It is intentionally ordinary TypeScript, HTML, and CSS. The
page owns its WebAssembly node exactly as a browser application would; it is
not a Dioxus integration or a general-purpose client.

The playground keeps both Auto Wi-Fi and USB Auto behind explicit clicks,
registers an LXMF delivery destination named `Prns Browser Playground`, and
exposes engine snapshots, single-packet payloads, and tagged outcomes for
inspection.

Build and stage the page into the documentation site's public assets:

```sh
npm --prefix prns-wasm run stage:docs
```

Serve the documentation public directory from the repository root:

```sh
python3 -m http.server 8878 --bind 127.0.0.1 --directory docs/website/public
```

Then open:

```text
http://127.0.0.1:8878/browser-node-playground-console/
```
