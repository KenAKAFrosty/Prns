import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';
import test from 'node:test';
import * as N from './fixtures/session-bindings.mjs';

const wrapperUrl = new URL('../src/index.native.ts', import.meta.url).href;
const moduleSource = source => `data:text/javascript,${encodeURIComponent(source)}`;
const imports = new Map([
  ['personal-rns/contract', moduleSource('export {};')],
  ['./bindings.native', new URL('./fixtures/session-bindings.mjs', import.meta.url).href],
  ['./generated/remote-control-adapter.generated', moduleSource('export const bindRemoteControlOperations = () => ({});')],
  ['./generated/host-adapter.generated', moduleSource('export const liftLifecycleSnapshot = value => value; export const lowerResourceCompression = value => value;')],
  ['./platform.native', moduleSource('export const defaultStoragePath = () => Promise.resolve("/storage");')],
  ['./stream', new URL('../src/stream.ts', import.meta.url).href],
]);
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    const url = context.parentURL === wrapperUrl ? imports.get(specifier) : undefined;
    return url === undefined ? nextResolve(specifier, context) : { url, shortCircuit: true };
  },
});
const { OwnedHostSession } = await import(wrapperUrl);
hooks.deregister();

function fixture(stop) {
  const native = new N.HostSession(stop);
  const session = new OwnedHostSession(native);
  session.host.applicationEvents();
  session.host.beginResourceUpload(new Uint8Array(16), 0n);
  return { native, session };
}

function assertReleased(native, session) {
  assert.equal(native.destroyed, 1);
  assert.equal(native.handle.destroyed, 1);
  assert.equal(native.handle.stream.closed, 1);
  assert.equal(native.handle.stream.destroyed, 1);
  assert.equal(native.handle.upload.aborted, 1);
  assert.equal(native.handle.upload.destroyed, 1);
  assert.throws(() => session.host.lifecycle(), /Host client released/);
}

function assertRetained(native, session) {
  assert.equal(native.destroyed, 0);
  assert.equal(native.handle.destroyed, 0);
  assert.equal(native.handle.stream.closed, 0);
  assert.equal(native.handle.upload.aborted, 0);
  assert.deepEqual(session.host.lifecycle(), { state: 'Running' });
}

test('successful stop shares completion and releases all session leases once', async () => {
  const { native, session } = fixture(() => Promise.resolve());
  const stopped = session.stop();
  assert.equal(session.stop(), stopped);
  await stopped;
  assertReleased(native, session);
  assert.equal(session.close(), stopped);
  assert.equal(native.stops, 1);
});

test('joined shutdown failure releases leases and preserves the same settled failure', async () => {
  const failure = new N.BindingError.Backend('EventBackpressure');
  const { native, session } = fixture(() => Promise.reject(failure));
  const stopped = session.stop();
  assert.equal(session.stop(), stopped);
  await assert.rejects(stopped, error => error === failure);
  assertReleased(native, session);
  assert.equal(session.close(), stopped);
  await assert.rejects(session.close(), error => error === failure);
  assert.equal(native.stops, 1);
  assertReleased(native, session);
});

for (const [name, failure] of [
  ['unavailable native ownership', new N.BindingError.OwnershipUnavailable('lifecycle unavailable')],
  ['unknown transport failure', new Error('JS runtime transport failure')],
  ['unrecognized error with a Backend tag', { tag: 'Backend' }],
]) {
  test(`${name} retains ownership and permits a later stop`, async () => {
    let attempt = 0;
    const { native, session } = fixture(() => ++attempt === 1 ? Promise.reject(failure) : Promise.resolve());
    const first = session.stop();
    await assert.rejects(first, error => error === failure);
    assertRetained(native, session);
    const retried = session.close();
    assert.notEqual(retried, first);
    await retried;
    assert.equal(native.stops, 2);
    assertReleased(native, session);
  });
}
