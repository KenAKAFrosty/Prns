import assert from 'node:assert/strict';
import test from 'node:test';
import {
  liftHostSnapshot, lowerHostSnapshot, liftSafeUint, lowerSafeUint,
} from '../typescript/host-adapter.generated.ts';

test('safe integers remain numbers and reject rounding in both directions', () => {
  for (const value of [0n, 1n, (1n << 53n) - 1n]) {
    assert.equal(lowerSafeUint(liftSafeUint(value)), value);
  }
  for (const value of [-1n, 1n << 53n, (1n << 64n) - 1n]) {
    assert.throws(() => liftSafeUint(value), RangeError);
  }
  for (const value of [-1, 0.5, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 53]) {
    assert.throws(() => lowerSafeUint(value), RangeError);
  }
});

test('canonical host snapshot survives transport with omitted optionals and exact counters', () => {
  const snapshot = {
    revision: (1n << 64n) - 1n,
    backend: { backend: 'Native', capabilities: ['Bluetooth'], interfaceKinds: ['AutomaticBluetoothLe'] },
    interfaces: [{
      interfaceId: new Uint8Array(8).fill(1), health: 'Connected',
      rxBytes: (1n << 64n) - 1n, txBytes: 1n << 53n,
      rxBps: 42, routeCount: 1, linkCount: 0, transportedLinkCount: 0,
    }],
    routes: [{
      destination: new Uint8Array(16).fill(2), hops: 1,
      interfaceId: new Uint8Array(8).fill(1),
      learnedAtMillis: 10, lastRouteActivityAtMillis: 11, expiresAtMillis: 100,
    }],
    activeLinkCount: 0,
    destinationIdentities: [{ destination: new Uint8Array(16).fill(2), identity: new Uint8Array(16).fill(3) }],
    runtime: {
      running: true, uptimeMillis: 13, interfaceCount: 1, onlineInterfaceCount: 1,
      routeCount: 1, linkCount: 0, transportedLinkCount: 0,
      rxBytes: (1n << 64n) - 1n, txBytes: 1n << 53n, rxBps: 42, txBps: 0,
    },
    persistence: { persistent: true, restored: true, lastFlushCause: 'Startup' },
  };
  const transport = lowerHostSnapshot(snapshot);
  // Generated Option readers may represent absence explicitly; the canonical
  // contract omits these properties under exactOptionalPropertyTypes.
  transport.interfaces[0].name = undefined;
  transport.routes[0].viaIdentity = undefined;
  transport.persistence.lastFailureDetail = undefined;
  const restored = liftHostSnapshot(transport);
  assert.deepEqual(restored, snapshot);
  assert.equal(Object.hasOwn(restored.interfaces[0], 'name'), false);
  assert.equal(Object.hasOwn(restored.routes[0], 'viaIdentity'), false);
  assert.equal(typeof restored.runtime.rxBytes, 'bigint');
  assert.equal(typeof restored.runtime.rxBps, 'number');
});
