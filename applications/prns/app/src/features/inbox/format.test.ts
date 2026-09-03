import type { LxmfDeliveryState, LxmfMessage } from "@prns-internal/expo";
import { destinationHash } from "personal-rns/contract";

import { deliveryLabel } from "./format";

const source = destinationHash(new Uint8Array(16).fill(0x11));
const destination = destinationHash(new Uint8Array(16).fill(0x22));

function message(deliveryState: LxmfDeliveryState): LxmfMessage {
  return {
    localRecordId: 1n,
    messageId: new Uint8Array(32).fill(0x33),
    source,
    destination,
    timestamp: 1_700_000_000_000n,
    title: { type: "utf8", value: "Title" },
    content: { type: "utf8", value: "Content" },
    direction: "outbound",
    verification: "verified",
    deliveryState,
  };
}

describe("durable LXMF delivery presentation", () => {
  test.each([
    [{ type: "received" }, "Received"],
    [{ type: "queued", failedAttempts: 0n }, "Queued"],
    [{ type: "queued", failedAttempts: 2n }, "Queued after 2 failed attempts"],
    [{ type: "sending", failedAttempts: 0n }, "Sending"],
    [{ type: "sending", failedAttempts: 1n }, "Sending after 1 failed attempt"],
    [
      { type: "failed", failedAttempts: 3n, lastFailure: "localNodeStopped" },
      "Failed after 3 failed attempts — this device went offline",
    ],
  ] satisfies readonly (readonly [LxmfDeliveryState, string])[])(
    "renders durable state %#",
    (state, expected) => {
      expect(deliveryLabel(message(state))).toBe(expected);
    },
  );

  test("shows delivery timing and terminal cancellation without transport jargon", () => {
    expect(
      deliveryLabel(message({ type: "delivered", deliveredAt: 1_700_000_000_123n, rtt: 23n })),
    ).toMatch(/^Delivered in 23 ms at /u);
    expect(
      deliveryLabel(message({ type: "delivered", deliveredAt: 1_700_000_000_123n, rtt: null })),
    ).toMatch(/^Delivered at /u);
    expect(deliveryLabel(message({ type: "cancelled", cancelledAt: 1_700_000_000_456n }))).toMatch(
      /^Cancelled /u,
    );
  });
});
