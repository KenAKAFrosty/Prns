# personal-rns

`personal-rns` provides one casework-shaped JavaScript API for native Node.js, Bun, and browsers.

The root export selects the native backend in Node.js and Bun and the cooperative WebAssembly backend in browser bundlers. Explicit `personal-rns/native` and `personal-rns/browser` subpaths are available when runtime selection must be fixed.

Application events and diagnostics are separate, bounded, single-owner streams. Claiming a stream is an explicit casework outcome, so an ownership conflict never appears as an iterator exception:

```ts
import { match } from "personal-rns";

match(node.claimEvents(), {
  Claimed: async (events) => {
    for await (const event of events) {
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
  },
  AlreadyClaimed: ({ lane }) => reportConsumerConflict(lane),
});
```

The compiler requires every declared case. Commands settle their returned promises, expected failures are tagged outcomes, and public binary values are semantically branded `Uint8Array` instances. A `ResourceAvailable` event owns a `ResourceStream`; its `claim()` method uses the same `Claimed | AlreadyClaimed` contract.
