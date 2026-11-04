import assert from "node:assert/strict";
import test from "node:test";
import { createEnrollmentWait, ENROLLMENT_WAIT_MILLIS } from "../src/enrollment-wait.js";

test("the default window exceeds the measured startup and explicit continuation renews it", async () => {
  let now = 0;
  const events = [];
  const wait = createEnrollmentWait({ now: () => now, onProgress: event => events.push(event) });
  assert.equal(ENROLLMENT_WAIT_MILLIS, 120_000);
  assert.equal(wait.continueChecking(), false);
  assert.equal(await wait.afterPending(), null);
  now = 30_000;
  assert.equal(await wait.afterPending(), null);
  assert.equal(events.at(-1).canContinue, true);
  assert.equal(wait.continueChecking(), true);
  now = 120_000;
  assert.equal(await wait.afterPending(), null);
  assert.equal(events.at(-1).paused, false);
  wait.close();
  assert.equal(wait.continueChecking(), false);
});

test("expiry pauses until explicit continuation and stop keeps an uncertain durable result pending", async () => {
  let now = 0;
  const events = [];
  const wait = createEnrollmentWait({ now: () => now, setupTimeoutMs: 100, onProgress: event => events.push(event) });
  await wait.afterPending();
  now = 100;
  let settled = false;
  const paused = wait.afterPending().then(result => { settled = true; return result; });
  await Promise.resolve();
  assert.equal(settled, false);
  assert.equal(events.at(-1).paused, true);
  assert.equal(wait.continueChecking(), true);
  assert.equal(await paused, null);
  now = 200;
  const pausedAgain = wait.afterPending();
  assert.equal(wait.stopChecking(), true);
  assert.equal((await pausedAgain).status, "pending");
  assert.equal(wait.continueChecking(), false);
});

test("a broken progress consumer stops checking rather than keeping an unobservable port open", async () => {
  const wait = createEnrollmentWait({ onProgress() { throw Error("view removed"); } });
  assert.equal((await wait.afterPending()).status, "pending");
});

test("Stop is latched before the first matching Pending receipt", async () => {
  const wait = createEnrollmentWait();
  assert.equal(wait.stopChecking(), true);
  assert.throws(() => wait.throwIfStopped(), /stopped/);
  assert.equal((await wait.afterPending()).status, "pending");
});

test("a rejected asynchronous progress delivery releases a paused waiter", async () => {
  let now = 0;
  const wait = createEnrollmentWait({ now: () => now, setupTimeoutMs: 100,
    onProgress(event) { return event.paused ? Promise.reject(Error("receiver gone")) : Promise.resolve(); },
  });
  await wait.afterPending();
  now = 100;
  assert.equal((await wait.afterPending()).status, "pending");
});
