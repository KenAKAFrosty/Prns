import assert from "node:assert/strict";
import { test } from "node:test";

import { Tag } from "../dist/casework.js";
import {
  byteResourceSource,
  sendResourceFromSource,
} from "../dist/browser/resource_send.js";
import { linkId } from "../dist/contract.js";

const LINK = linkId(new Uint8Array(16).fill(4));

test("browser resource sends keep two segments in flight and settle only after the final proof", async () => {
  const settlements = [];
  const issued = [];
  const settled = [];
  const source = byteResourceSource(
    new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
  );
  const sending = sendResourceFromSource(
    LINK,
    source,
    Tag("Never"),
    new Uint8Array([10, 11]),
    {
      maximumInFlightSegments: 2,
      plan: ({ segmentIndex }) => plan(segmentIndex),
      compress: async () => undefined,
      issue: (input) => {
        issued.push(input);
        return new Promise((resolve) => {
          settlements.push(resolve);
        });
      },
    },
  ).then((outcome) => {
    settled.push(outcome);
    return outcome;
  });

  await eventTurn();
  assert.equal(issued.length, 2);
  assert.equal(settled.length, 0);
  assert.deepEqual(
    issued.map((segment) => segment.payload),
    [
      new Uint8Array([0, 1, 2, 3]),
      new Uint8Array([4, 5, 6]),
    ],
  );
  assert.deepEqual(issued[0], {
    totalDataBytes: 10,
    segmentIndex: 1,
    metadata: "packed",
    packedMetadata: new Uint8Array([10, 11]),
    linkId: LINK,
    payload: new Uint8Array([0, 1, 2, 3]),
  });
  assert.deepEqual(issued[1], {
    totalDataBytes: 10,
    segmentIndex: 2,
    packedMetadataBytes: 2,
    metadata: "sentInFirstSegment",
    linkId: LINK,
    payload: new Uint8Array([4, 5, 6]),
  });

  settlements[0](Tag("Succeeded", Tag("ResourceSent")));
  await eventTurn();
  assert.equal(issued.length, 3);
  assert.equal(settled.length, 0);
  assert.deepEqual(issued[2], {
    totalDataBytes: 10,
    segmentIndex: 3,
    packedMetadataBytes: 2,
    metadata: "sentInFirstSegment",
    linkId: LINK,
    payload: new Uint8Array([7, 8, 9]),
  });

  settlements[1](Tag("Succeeded", Tag("ResourceSent")));
  await eventTurn();
  assert.equal(settled.length, 0);
  settlements[2](Tag("Succeeded", Tag("ResourceSent")));
  assert.deepEqual(
    await sending,
    Tag("Succeeded", Tag("ResourceSent")),
  );
});

test("browser resource sends preserve the first exact segment failure", async () => {
  const settlements = [];
  const issued = [];
  const sending = sendResourceFromSource(
    LINK,
    byteResourceSource(new Uint8Array(10)),
    Tag("Never"),
    undefined,
    {
      maximumInFlightSegments: 2,
      plan: ({ segmentIndex }) => plan(segmentIndex),
      compress: async () => undefined,
      issue: (input) => {
        issued.push(input);
        return new Promise((resolve) => {
          settlements.push(resolve);
        });
      },
    },
  );

  await eventTurn();
  assert.equal(issued.length, 2);
  settlements[0](Tag("Failed", Tag("ResourceRejectedByPeer")));
  assert.deepEqual(
    await sending,
    Tag("Failed", Tag("ResourceRejectedByPeer")),
  );
  await eventTurn();
  assert.equal(issued.length, 2);
});

test("browser resource sends issue the first segment while preparing the second", async () => {
  const issued = [];
  const settlements = [];
  let provideSecond;
  const second = new Promise((resolve) => {
    provideSecond = resolve;
  });
  const sending = sendResourceFromSource(
    LINK,
    {
      totalBytes: 10,
      read: async (dataStart, dataEnd) =>
        dataStart === 4
          ? second
          : new Uint8Array(dataEnd - dataStart),
    },
    Tag("Never"),
    undefined,
    {
      maximumInFlightSegments: 2,
      plan: ({ segmentIndex }) => plan(segmentIndex),
      compress: async () => undefined,
      issue: (input) => {
        issued.push(input);
        return new Promise((resolve) => {
          settlements.push(resolve);
        });
      },
    },
  );

  await eventTurn();
  assert.equal(issued.length, 1);
  provideSecond(new Uint8Array(3));
  await eventTurn();
  assert.equal(issued.length, 2);
  settlements[0](Tag("Failed", Tag("ResourceRejectedByPeer")));
  assert.deepEqual(
    await sending,
    Tag("Failed", Tag("ResourceRejectedByPeer")),
  );
});

test("browser resource plan rejections remain typed", async () => {
  const metadata = await sendResourceFromSource(
    LINK,
    byteResourceSource(new Uint8Array(1)),
    Tag("Never"),
    new Uint8Array(1),
    {
      maximumInFlightSegments: 2,
      plan: () => ({
        type: "rejected",
        cause: "metadataTooLarge",
      }),
      compress: async () => undefined,
      issue: async () => Tag("Succeeded", Tag("ResourceSent")),
    },
  );
  assert.deepEqual(
    metadata,
    Tag("Failed", Tag("ResourceMetadataTooLarge")),
  );
});

test("browser resource source and driver failures settle instead of rejecting", async () => {
  const sourceFailure = await sendResourceFromSource(
    LINK,
    {
      totalBytes: 10,
      read: async () => {
        throw new Error("source unavailable");
      },
    },
    Tag("Never"),
    undefined,
    {
      maximumInFlightSegments: 2,
      plan: ({ segmentIndex }) => plan(segmentIndex),
      compress: async () => undefined,
      issue: async () => Tag("Succeeded", Tag("ResourceSent")),
    },
  );
  assert.deepEqual(
    sourceFailure,
    Tag("Failed", Tag("WriteFailed", { detail: "source unavailable" })),
  );

  const driverFailure = await sendResourceFromSource(
    LINK,
    byteResourceSource(new Uint8Array([1, 2, 3, 4])),
    Tag("Never"),
    undefined,
    {
      maximumInFlightSegments: 1,
      plan: () => ({
        type: "ready",
        totalStreamBytes: 4,
        segmentIndex: 1,
        totalSegments: 1,
        totalDataBytes: 4,
        dataStart: 0,
        dataEnd: 4,
        streamBytes: 4,
      }),
      compress: async () => undefined,
      issue: async () => {
        throw new Error("driver unavailable");
      },
    },
  );
  assert.deepEqual(
    driverFailure,
    Tag("Failed", Tag("WriteFailed", { detail: "driver unavailable" })),
  );
});

function plan(segmentIndex) {
  const ranges = [
    [0, 4],
    [4, 7],
    [7, 10],
  ];
  const range = ranges[segmentIndex - 1];
  assert.ok(range);
  return {
    type: "ready",
    totalStreamBytes: 15,
    segmentIndex,
    totalSegments: 3,
    totalDataBytes: 10,
    dataStart: range[0],
    dataEnd: range[1],
    streamBytes: range[1] - range[0],
  };
}

async function eventTurn() {
  await new Promise((resolve) => setImmediate(resolve));
}
