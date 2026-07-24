# personal-rns

Node.js bindings for [Personal Reticulum](https://prns.dev) — a fast, robust implementation of the [Reticulum](https://reticulum.network) network stack. The full engine runs in-process as a native addon: your Node service or Electron/Tauri app becomes a real Reticulum node, no daemon required.

```
npm install personal-rns
```

Prebuilt binaries ship for Windows (x64, arm64), macOS (x64, arm64), and Linux (x64, arm64 glibc; x64 musl). Node.js >= 20; Bun works out of the box. Wire parity with reference Reticulum 1.4.0 is proven by interop suites in both server and client roles.

## Quickstart

```js
const { startNode, requestPathHash } = require('personal-rns');

async function main() {
  const node = startNode(
    {
      destinations: [
        {
          appName: 'example',
          aspects: ['host'],
          identity: { path: './identity' },
          requestPaths: [{ path: '/echo' }],
        },
      ],
    },
    (event) => {
      if (event.type === 'request') {
        node.respond(event.token, Buffer.concat([Buffer.from('echo:'), event.data]));
      }
    }
  );
  await node.ready();

  await node.attachTcpServer({ bind: '0.0.0.0:4242' });
  const [myDestination] = node.destinationHashes;
  setInterval(() => node.announce(myDestination), 60_000);
  await node.announce(myDestination);
}

main();
```

A client on another machine:

```js
const { startNode, requestPathHash } = require('personal-rns');

const heard = [];
const node = startNode({}, (event) => heard.push(event));
await node.ready();
await node.attachTcpClient({ target: 'server.example.com:4242' });

const announce = await untilAnnounce(heard);
const linkId = await node.establishLink(announce.destination);
const { data } = await node.request(linkId, requestPathHash('/echo'), Buffer.from('hi'));
console.log(data.toString());
```

## Model

- `startNode(options, onEvent)` starts the engine on a dedicated native thread and returns immediately; `await node.ready()` completes startup. The engine owns its sockets and keeps the process alive until `await node.stop()`.
- Every event arrives through the single `onEvent` callback as a plain object with a `type` tag (`announce`, `singleDelivery`, `linkEstablished`, `request`, `response`, `resourceReceived`, `nodeStopped`, …). Byte fields are `Buffer`s.
- All failures throw (or reject) with a stable machine-readable `error.code` such as `PRNS_NODE_STOPPED`, `PRNS_LINK_TIMEOUT`, `PRNS_REQUEST_FAILED`, `PRNS_INVALID_ARGUMENT`.
- Identities: pass `{ path }` to keep secrets in a file managed natively (recommended), or `{ secret: Buffer }` for full control. `destinationHashes` exposes your addresses.

## Interfaces

Programmatic constructors cover the common families:

| Method | Family |
| --- | --- |
| `attachTcpServer({ bind })` / `attachTcpClient({ target })` | TCP |
| `attachUdp({ local, peer })` | UDP |
| `attachSharedInstanceServer({ port? })` / `attachSharedInstanceClient({ port? })` | RNS shared instance |
| `attachAutoWifi()` / `attachAutoBluetoothLe({ identityPath })` / `attachAutoUsb({ baud? })` | Auto discovery radios |

`attachConfig(configText)` accepts the `[interfaces]` section of a standard RNS config file and stands up every interface it declares — TCP, UDP, serial, KISS, AX.25, RNode, backbone, WebSocket, I2P, and the auto radio families — through the same code paths the `prnsd` daemon uses.

## Beyond packets

- **Links and requests**: `establishLink`, `request(linkId, requestPathHash(path), data)`, and server-side `requestPaths` registration with `respond(event.token, bytes)` / `respondFile` — the RNS request/response pattern end to end.
- **Resources**: `sendResource(File)` and `receiveResource(File)` move payloads of any size over a link with metadata, optional compression control, and `resourceSendProgress` events; byte quantities use explicit names such as `totalSizeBytes` and `transferredBytes`, and acceptance is governed per destination or per link via `resourceStrategy`.
- **Observability**: `interfaces()`, `routes()`, `linkCount()`, `announceRates()`, and `destinationIdentity()` read the live node; `dropRoute`, `clearAnnounceQueues`, and the blackhole and retention families manage it.
- **Backpressure**: a slow event handler can never stall the engine — past `eventQueueLimit` the node sheds diagnostic events and reports the gap with a single `eventOverflow` event, while data-plane events always deliver.

Public quantities carry their unit in the property name: byte counts use `*Bytes`,
and durations or timestamps use `*Millis`. The older unsuffixed and `*Ms`
properties remain accepted or emitted as compatibility aliases throughout the
0.2.x series; new code should use the unit-bearing names. `attachAutoBle` is
likewise retained as an alias for the canonical `attachAutoBluetoothLe` name.

## License

MIT OR Apache-2.0
