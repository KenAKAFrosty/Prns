# personal-rns

`personal-rns` provides one JavaScript/TypeScript API for native Node.js, Bun, and browsers.

The root export selects the native backend in Node.js and Bun and the cooperative WebAssembly backend in browser bundlers. Explicit `personal-rns/native` and `personal-rns/browser` subpaths are available when runtime selection must be fixed.

## Install

For a published registry release:

```console
npm install personal-rns
```

Prns 0.3.7 is available as a public GitHub prerelease. Registry publication has an independent qualification gate, so use the [source-checkout instructions](../docs/sdks.md#typescript-and-javascript) when you need the exact candidate before that gate completes.

## Create a host

Node.js and Bun use the native backend selected by the root export:

```ts
import { Prns, Tag } from "personal-rns";

const created = await Prns.create({
  identity: Tag("GenerateEphemeral"),
  role: "Endpoint",
});
if (created.tag !== "Ready") {
  throw new Error(`node creation failed: ${created.tag}`);
}

const node = created.data;
console.log(node.identityHash);
await node.stop();
```

Browsers use the cooperative WebAssembly backend:

```ts
import { Prns } from "personal-rns/browser";

const created = await Prns.create({});
if (created.tag !== "Ready") {
  throw new Error(`browser node creation failed: ${created.tag}`);
}

const node = created.data;
console.log(node.backendInfo);
await node.stop();
```

Web Bluetooth connects the browser as a GATT central to a native or embedded
Prns node advertising the shared Bluetooth Auto service. Start the chooser
from a user action in a supported secure-context browser:

```ts
connectButton.addEventListener("click", async () => {
  const connected = await node.interfaces.bluetooth.connect();
  if (connected.tag !== "Connected") {
    reportBluetoothFailure(connected);
    return;
  }

  const session = connected.data;
  showInterface(session.interfaceId);
});
```

The session carries Reticulum packets in both directions over the shared GATT
control and data characteristics. Browser instances do not advertise this
service, so a browser-to-browser Bluetooth link still requires a native or
embedded Prns transport node nearby.

## Handle events and commands

Application events and diagnostics are separate, bounded, single-owner streams. Claiming a stream is an explicit outcome, so an ownership conflict never appears as an iterator exception. Handle that boundary once, then keep the event loop flat:

```ts
import { match } from "personal-rns";

const claim = node.claimEvents();
if (claim.tag === "AlreadyClaimed") {
  reportConsumerConflict(claim.data.lane);
  return;
}

for await (const event of claim.data) {
  match(event, {
    SingleDelivery: ({ destination, plaintext, sourceInterface }) => {
      receiveSingle(destination, plaintext, sourceInterface);
    },
    LinkDelivery: ({ linkId, plaintext, sourceInterface }) => {
      receiveLinkPacket(linkId, plaintext, sourceInterface);
    },
    Request: receiveRequest,
    Response: receiveResponse,
    ResponseSegment: receiveResponseSegment,
    ResourceAvailable: receiveResource,
    ResourceSegment: receiveResourceSegment,
    ResourceNeedsDecompression: provideDecompressedResource,
    ChannelMessage: receiveChannelMessage,
  });
}
```

Host-to-node control uses the same generated `HostCommand` and `CommandSettlement` sums in Node.js, Bun, and browsers:

```ts
import { Tag, match } from "personal-rns";

const settlement = await node.execute(
  Tag("SendSinglePacket", { destination, payload }),
);
if (settlement.tag === "Failed") {
  reportCommandFailure(settlement.data);
  return;
}

match(settlement.data, {
  Announced: confirmAnnounce,
  PacketDelivered: confirmDelivery,
  LinkCloseQueued: confirmLinkClose,
  InterfaceAttached: rememberInterface,
  InterfaceDetached: forgetInterface,
  LinkEstablished: rememberLink,
  PathDiscovered: rememberPath,
  Identified: confirmIdentity,
  ResponseReceived: receiveResponse,
  ResponseSent: confirmResponse,
  ResourceSent: confirmResource,
  ResourceStrategySet: confirmResourceStrategy,
  RequesterAllowed: confirmRequester,
});
```
The compiler requires every declared case. Commands settle their returned promises, expected failures are typed tagged outcomes, and public binary values are semantically branded `Uint8Array` instances. Browser backends attach `WebSocketClient` and `BrowserRendezvous` through the bounded cooperative transport and return `UnsupportedByBackend` for native-only interface kinds. Each host reports its current support through `backendInfo` and `capabilities`. The browser `hostSnapshot()` projects the generated inspection contract with revisioned routes, destination identities, logical interfaces, transfer counters, runtime health, and exact persistence status. A `ResourceAvailable` event owns a `ResourceStream`; its `claim()` method uses the same `Claimed | AlreadyClaimed` contract.

Browser hosts are ephemeral by default. `persistentBrowser()` selects a caller-named `localStorage` root for the host identity, Bluetooth identity, routing state, destination identities, tunnels, and ratchets. Interfaces remain caller-supplied after restart. `stop()` flushes the bounded state before settling, while restoration and flush results appear on the diagnostic stream and in `hostSnapshot()`:

```ts
import { Prns, persistentBrowser } from "personal-rns/browser";

const created = await Prns.create(persistentBrowser("my-app"));
if (created.tag !== "Ready") {
  reportCreationFailure(created);
  return;
}

const node = created.data;
await attachApplicationInterfaces(node);
await runApplication(node);

const stopped = await node.stop();
if (stopped.tag !== "Stopped") {
  reportShutdownFailure(stopped);
}
```

Sending a Resource in the browser accepts either bytes or a `Blob`. The `Blob` path slices the source into bounded segments instead of materializing the whole value:

```ts
import { Tag, match } from "personal-rns/browser";

const sent = await node.sendResourceBlob(link, file, {
  compression: Tag("Auto"),
  packedMetadata,
});
if (sent.tag === "Failed") {
  reportResourceFailure(sent.data);
  return;
}

match(sent.data, {
  ResourceSent: confirmResource,
});
```

`Auto` compression runs the shared Rust codec in a dedicated module Worker. The send remains correct if Worker startup or compression is unavailable: it continues with the uncompressed segment. Planning, metadata placement, segment bounds, and wire submission remain in the shared Rust implementation.

## More examples

[`examples/native-lifecycle.ts`](examples/native-lifecycle.ts) is a complete native lifecycle program with a self-contained loopback interface. The [browser transport playground](../prns-wasm/examples/browser-playground/README.md) runs a live node with permission-gated Web Bluetooth, WebUSB, and Wi-Fi controls.
