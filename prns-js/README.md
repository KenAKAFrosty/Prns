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
});
```

The compiler requires every declared case. Commands settle their returned promises, expected failures are typed tagged outcomes, and public binary values are semantically branded `Uint8Array` instances. Browser backends return `UnsupportedByBackend` for native interface attachment commands instead of omitting or weakening the common API. A `ResourceAvailable` event owns a `ResourceStream`; its `claim()` method uses the same `Claimed | AlreadyClaimed` contract.
