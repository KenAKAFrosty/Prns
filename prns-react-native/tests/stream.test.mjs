import test from 'node:test';
import assert from 'node:assert/strict';
import { HostEventIterator } from '../src/stream.ts';

function lane() {
  let resolve;
  let reject;
  const state = { drained: 0, closed: 0 };
  const native = {
    ready({ signal }) {
      return new Promise((yes, no) => {
        resolve = yes; reject = no;
        signal.addEventListener('abort', () => no(new Error('aborted')), { once: true });
      });
    },
    tryNext() { state.drained++; return 42; },
    close() { state.closed++; },
  };
  return { native, state, wake: () => resolve(true), fail: () => reject(new Error('native failure')) };
}

test('aborting a readiness wait releases the claim without dequeuing', async () => {
  const fixture = lane(); const abort = new AbortController();
  const iterator = new HostEventIterator(fixture.native, value => value, abort.signal);
  const pending = iterator.next(); abort.abort();
  await assert.rejects(pending, /abort/);
  assert.equal(fixture.state.drained, 0); assert.equal(fixture.state.closed, 1);
});

test('abort after readiness but before the microtask drain does not consume', async () => {
  const fixture = lane(); const abort = new AbortController();
  const iterator = new HostEventIterator(fixture.native, value => value, abort.signal);
  const pending = iterator.next(); fixture.wake(); abort.abort();
  await assert.rejects(pending, { name: 'AbortError' });
  assert.equal(fixture.state.drained, 0); assert.equal(fixture.state.closed, 1);
});

test('a live iterator drains exactly once and closes on return', async () => {
  const fixture = lane(); const iterator = new HostEventIterator(fixture.native, value => value + 1);
  const pending = iterator.next(); fixture.wake();
  assert.deepEqual(await pending, { done: false, value: 43 });
  await iterator.return(); await iterator.return();
  assert.equal(fixture.state.drained, 1); assert.equal(fixture.state.closed, 1);
});

test('return quiesces a pending wait and prevents overlapping next calls', async () => {
  const fixture = lane(); const iterator = new HostEventIterator(fixture.native, value => value);
  const pending = iterator.next();
  await assert.rejects(iterator.next(), /Only one next/);
  await iterator.return();
  assert.deepEqual(await pending, { done: true, value: undefined });
  assert.equal(fixture.state.drained, 0);
});

test('native readiness failure closes the consumer claim', async () => {
  const fixture = lane(); const iterator = new HostEventIterator(fixture.native, value => value);
  const pending = iterator.next(); fixture.fail();
  await assert.rejects(pending, /native failure/);
  assert.equal(fixture.state.closed, 1);
});
