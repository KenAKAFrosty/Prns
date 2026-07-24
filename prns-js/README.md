# personal-rns

`personal-rns` provides one casework-shaped JavaScript API for native Node.js, Bun, and browsers.

The root export selects the native backend in Node.js and Bun and the cooperative WebAssembly backend in browser bundlers. Explicit `personal-rns/native` and `personal-rns/browser` subpaths are available when runtime selection must be fixed.

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

The compiler requires every declared case. Commands settle their returned promises, expected failures are typed tagged outcomes, and public binary values are semantically branded `Uint8Array` instances. Browser backends return `UnsupportedByBackend` for native interface attachment commands instead of omitting or weakening the common API. A `ResourceAvailable` event owns a `ResourceStream`; its `claim()` method uses the same `Claimed | AlreadyClaimed` contract.

Browser resource sends accept either bytes or a `Blob`. The `Blob` path slices
the source into bounded segments instead of materializing the whole value:

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

`Auto` compression runs the shared Rust codec in a dedicated module Worker.
The send remains correct if Worker startup or compression is unavailable: it
continues with the uncompressed segment. Planning, metadata placement, segment
bounds, and wire submission remain in the shared Rust implementation.
