# Local Hopspot browser hosting

This development bundle serves the existing browser transport playground from
an OpenWrt device. The page owns a real browser node. Its WebSocket field is
prefilled with the same host and the port selected at build time; the user still
clicks Connect. The WebSocket listener and static HTTP server are separate from
administration. This does not install a persistent service or change Wi-Fi.

## Build

```sh
./tools/prns run build.hopspot.browser -- \
  --output /tmp/hopspot-browser-bundle --websocket-port 4344
```

The build uses the current WASM engine and browser SDK together, copies the local
favicon and license notices, and produces per-file SHA-256 hashes and size/build
metadata. `public/` is the HTTP document root. Install the locked dependencies in
`prns-js` and `prns-wasm` before building. The G4 application is built separately
with `build.hopspot.g4 --with-websocket`; it must listen on the selected port.

## Temporary G4 hosting

Use an unused port and the board's actual LAN address. Verify the bundle hashes
before copying only `public/` into a fresh private directory under `/tmp`. Keep
identity/state directories outside that public root. Do not replace LuCI's files,
configuration, or listener. Run a separate, bounded HTTP-server process with a
private empty configuration file, directory listings disabled, and no symlink
escape. The inspected vendor image includes `/usr/sbin/uhttpd`.

For a prepared root at `/tmp/hopspot-browser/public`, run in a managed foreground
session (or supervise it with a process-specific cleanup watchdog):

```sh
: > /tmp/hopspot-browser/httpd.conf
/usr/sbin/uhttpd -f -p 192.168.12.1:8082 \
  -h /tmp/hopspot-browser/public -c /tmp/hopspot-browser/httpd.conf \
  -x /__no_cgi__ -D -S -N 16 -T 15 -k 5
```

The supplied assets contain no CGI programs; do not put executables into that
root. The empty server config avoids inheriting management handlers. This
command does not register UBUS, Lua, or ucode handlers. Stop this process at the
end of the lab session; do not kill unrelated `uhttpd` processes.

Start a bounded Hopspot separately, with its private state outside `public/`:

```sh
./personal-hopspot-headless --state-dir /tmp/hopspot-browser/state \
  --listen 192.168.12.1:4343 --websocket-listen 192.168.12.1:4344 \
  --websocket-connections 8 --run-for 180
```

Open `http://192.168.12.1:8082/`, wait for the browser node, and click Connect
WebSocket. Add the already-qualified HaLoW arguments/profile when testing remote
radio destinations. Browser refresh destroys the active session, while the
playground's origin-scoped browser identity storage retains its identity. Changing
the HTTP origin, clearing browser storage, or private browsing can change that
identity.

## Scope and storage

Plain LAN HTTP is an insecure origin. A real Chrome test fetched the full Hopspot
page with `isSecureContext=false` and `crypto.subtle` unavailable, using Prns's
portable crypto path. This does not make secure-context-only Web Bluetooth,
WebUSB, service workers, or every browser API available. Other browsers require
separate qualification. The browser assets and wire endpoint use no external
CDN; Reticulum link encryption does not authenticate the HTTP-served application
code. Trusted HTTPS provisioning remains a distinct deployment option.

The uncompressed browser assets plus the WebSocket/HaLoW executable exceed the
observed free G4 overlay budget before retained state and update rollback. Keep
this deployment in RAM for now. Do not treat a compressed transfer archive as its
installed footprint. The bundle metadata records actual public-file bytes;
filesystem overhead and identities, logs, updates, and rollback need additional
space. Persistent storage and an update strategy remain unqualified.

## Repeat the qualification

With the documentation site's Playwright dependency and Chrome installed:

```sh
PRNS_BROWSER_CHANNEL=chrome node personal-hopspot/headless/scripts/browser-ui-smoke.mjs \
  http://192.168.12.1:8082/ ws://192.168.12.1:4344/

PRNS_BROWSER_CHANNEL=chrome PRNS_BROWSER_ORIGIN=http://192.168.12.1:8082/ \
  node personal-hopspot/headless/scripts/websocket-browser-smoke.mjs \
  /tmp/hopspot-browser-bundle/public "$EXPECTED_PAGE_FILE" \
  ws://192.168.12.1:4344/ "$G4_DESTINATION" "$HELTEC_DESTINATION"
```

`EXPECTED_PAGE_FILE` is `hopspot_index_no_source.mu` from the tested application
build's `personal-hopspot-core` OUT_DIR. The transfer smoke compares the entire
packed response, records the origin's security capabilities, and blocks HTTP
requests outside the selected board origin. The UI smoke checks endpoint
prefilling, Connect/Close, and identity retention on reload. See the
[local-hosting qualification](qualification/browser-local-hosting-2026-09-29.md)
for measurements and limits.
