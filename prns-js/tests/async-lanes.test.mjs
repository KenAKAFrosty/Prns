import assert from "node:assert/strict";
import { test } from "node:test";

import {
  BoundedAsyncLane,
} from "../dist/async_lanes.js";
import { match } from "../dist/casework.js";

test("application pressure rejects without losing accepted values", async () => {
  let rejectedBytes = 0;
  const lane = new BoundedAsyncLane({
    name: "ApplicationEvents",
    maximumValues: 2,
    maximumBytes: 3,
    measure: (value) => value.length,
    onRejected: (bytes) => {
      rejectedBytes = bytes;
    },
  });

  assert.equal(lane.push("aa"), "Queued");
  assert.equal(lane.push("bb"), "Rejected");
  assert.equal(rejectedBytes, 2);
  const iterator = match(lane.claim(), {
    Claimed: (stream) => stream,
    AlreadyClaimed: () => assert.fail("first claim was unavailable"),
  });
  assert.deepEqual(await iterator.next(), { done: false, value: "aa" });
  assert.deepEqual(
    lane.claim(),
    {
      tag: "AlreadyClaimed",
      data: { lane: "ApplicationEvents" },
    },
  );
});

test("diagnostic pressure drops newest and reports one exact gap", async () => {
  const lane = new BoundedAsyncLane({
    name: "Diagnostics",
    maximumValues: 1,
    maximumBytes: Number.MAX_SAFE_INTEGER,
    measure: () => 0,
    gap: (count) => ({ gap: count }),
  });

  assert.equal(lane.push({ event: 1 }), "Queued");
  assert.equal(lane.push({ event: 2 }), "Dropped");
  assert.equal(lane.push({ event: 3 }), "Dropped");
  const iterator = match(lane.claim(), {
    Claimed: (stream) => stream,
    AlreadyClaimed: () => assert.fail("diagnostic claim was unavailable"),
  });
  assert.deepEqual(await iterator.next(), {
    done: false,
    value: { event: 1 },
  });
  assert.deepEqual(await iterator.next(), {
    done: false,
    value: { gap: 2n },
  });
});

test("concurrent next calls remain ordered", async () => {
  const lane = new BoundedAsyncLane({
    name: "Resource",
    maximumValues: 3,
    maximumBytes: 3,
    measure: () => 1,
  });
  lane.push(1);
  lane.push(2);
  lane.finish();
  const iterator = match(lane.claim(), {
    Claimed: (stream) => stream,
    AlreadyClaimed: () => assert.fail("resource claim was unavailable"),
  });

  assert.deepEqual(
    await Promise.all([iterator.next(), iterator.next(), iterator.next()]),
    [
      { done: false, value: 1 },
      { done: false, value: 2 },
      { done: true, value: undefined },
    ],
  );
});
