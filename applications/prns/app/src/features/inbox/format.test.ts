import * as Bindings from "@prns-internal/expo";
import type { LxmfDeliveryState, LxmfMessage } from "@prns-internal/expo";
import { destinationHash } from "personal-rns/contract";
import { deliveryLabel, peerLabel } from "./format";
import { lastHeardLabel } from "@/features/contacts/messaging-directory";
const source = destinationHash(new Uint8Array(16).fill(0x11));
const destination = destinationHash(new Uint8Array(16).fill(0x22));
function message(deliveryState: LxmfDeliveryState): LxmfMessage {
  return {
    localRecordId: 1n,
    messageId: new Uint8Array(32).fill(0x33),
    source,
    destination,
    timestamp: 1700000000000n,
    title: Bindings.LxmfText.Utf8.new({
      value: "Title",
    }),
    content: Bindings.LxmfText.Utf8.new({
      value: "Content",
    }),
    direction: Bindings.LxmfDirection.Outbound,
    verification: Bindings.LxmfVerification.Verified,
    deliveryState,
  };
}
describe("durable LXMF delivery presentation", () => {
  test.each([
    [Bindings.LxmfDeliveryState.Received.new(), "Received"],
    [
      Bindings.LxmfDeliveryState.Queued.new({
        failedAttempts: 0n,
      }),
      "Queued",
    ],
    [
      Bindings.LxmfDeliveryState.Queued.new({
        failedAttempts: 2n,
      }),
      "Queued after 2 failed attempts",
    ],
    [
      Bindings.LxmfDeliveryState.Sending.new({
        failedAttempts: 0n,
      }),
      "Sending",
    ],
    [
      Bindings.LxmfDeliveryState.Sending.new({
        failedAttempts: 1n,
      }),
      "Sending after 1 failed attempt",
    ],
    [
      Bindings.LxmfDeliveryState.Failed.new({
        failedAttempts: 3n,
        lastFailure: Bindings.LxmfDeliveryFailure.LocalNodeStopped,
      }),
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
      deliveryLabel(
        message(
          Bindings.LxmfDeliveryState.Delivered.new({
            deliveredAt: 1700000000123n,
            rtt: 23n,
          }),
        ),
      ),
    ).toMatch(/^Delivered in 23 ms at /u);
    expect(
      deliveryLabel(
        message(
          Bindings.LxmfDeliveryState.Delivered.new({
            deliveredAt: 1700000000123n,
            rtt: undefined,
          }),
        ),
      ),
    ).toMatch(/^Delivered at /u);
    expect(
      deliveryLabel(
        message(
          Bindings.LxmfDeliveryState.Cancelled.new({
            cancelledAt: 1700000000456n,
          }),
        ),
      ),
    ).toMatch(/^Cancelled /u);
  });
});

test("retains private and durable announced names before ephemeral discovery names", () => {
  const peer: Bindings.LxmfPeerSummary = {
    destination,
    identity: source,
    displayName: "Live name",
    requiredStampCost: undefined,
    sourceInterface: new Uint8Array(16),
    hops: 1,
    isPathResponse: false,
    lastObservedAgeMillis: 0n,
  };
  const contact: Bindings.Contact = {
    destination,
    identity: source,
    alias: "Private name",
    announcedName: "Saved announced name",
    pinned: false,
    isMessaging: true,
  };
  expect(peerLabel(destination, [peer], [contact])).toBe("Private name");
  expect(peerLabel(destination, [], [{ ...contact, alias: undefined }])).toBe(
    "Saved announced name",
  );
  expect(peerLabel(destination, [peer], [])).toBe("Live name");
});

test.each([
  [0n, "Heard just now"],
  [59_999n, "Heard just now"],
  [60_000n, "Heard 1 minute ago"],
  [120_000n, "Heard 2 minutes ago"],
  [3_600_000n, "Heard 1 hour ago"],
  [7_200_000n, "Heard 2 hours ago"],
] as const)("renders a human readable discovery age %#", (age, label) => {
  expect(lastHeardLabel(age)).toBe(label);
});
