# personal-rns

Node.js bindings for [Personal Reticulum](https://prns.dev) — a fast, robust implementation of the [Reticulum](https://reticulum.network) network stack. The full engine runs in-process as a native addon: your Node service or Electron/Tauri app becomes a real Reticulum node, no daemon required.

```
npm install personal-rns
```

Prebuilt binaries ship for Windows (x64, arm64), macOS (x64, arm64), and Linux (x64, arm64 glibc; x64 musl). Node.js >= 20.

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

`attachConfig(configText)` accepts the `[interfaces]` section of a standard RNS config file and stands up every interface it declares — TCP, UDP, serial, KISS, AX.25, RNode, backbone, WebSocket, I2P, and the auto radio families — through the same code paths the `prnsd` daemon uses.

## License

MIT OR Apache-2.0
