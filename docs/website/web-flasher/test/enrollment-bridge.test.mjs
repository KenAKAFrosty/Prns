import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// Exercise the actual Rust-owned eval script: its recv consumer cannot observe
// an eval rejection, so every startup failure must send a terminal event.
const source = readFileSync(new URL("../../src/pages/flash/enrollment.rs", import.meta.url), "utf8");
const script = source.match(/const SCRIPT: &str = r#"([\s\S]*?)"#;/)[1];
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const run = new AsyncFunction("dioxus", "window", script);

test("a rejected bridge import sends Unknown to the Rust recv loop", async () => {
  const events = [];
  await run({ recv: async () => ({}), send: event => events.push(event) }, {});
  assert.equal(events.length, 1);
  assert.equal(events[0].status, "unknown");
});

test("an unexpected bridge failure sends Unknown without stopping a newer operation", async () => {
  const events = [];
  await run({ recv: async () => ({}), send: event => events.push(event) }, {
    __prnsFlash: {
      enrollController: async () => { throw Error("unexpected bridge failure"); },
      stopControllerEnrollmentCheck() { assert.fail("old script must not stop another operation"); },
    },
  });
  assert.deepEqual(events.map(event => event.status), ["unknown"]);
});

test("progress is delivered before terminal completion and its delivery promise is returned", async () => {
  const events = [];
  let finish;
  const completion = new Promise(resolve => { finish = resolve; });
  let progressSent;
  const progress = new Promise(resolve => { progressSent = resolve; });
  const delivery = Promise.resolve();
  const running = run({ recv: async () => ({}), send(event) { events.push(event); return delivery; } }, {
    __prnsFlash: {
      async enrollController(_request, { onProgress }) {
        assert.equal(onProgress({ status: "waiting", canContinue: true, paused: true, message: "Still pending" }), delivery);
        progressSent();
        return completion;
      },
    },
  });
  await progress;
  assert.deepEqual(events.map(event => event.status), ["waiting"]);
  finish({ status: "pending", message: "Checking stopped" });
  await running;
  assert.deepEqual(events.map(event => event.status), ["waiting", "pending"]);
});
