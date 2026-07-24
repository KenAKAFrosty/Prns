import assert from 'node:assert/strict';
import { test } from 'node:test';

import { startNode } from '../../index.js';
import { sleep, waitFor, type AnyEvent } from './helpers.js';

test('start/stop soak holds up over repeated cycles', async () => {
  for (let i = 0; i < 25; i += 1) {
    const events: AnyEvent[] = [];
    const node = startNode(
      { destinations: [{ appName: 'prnsnapi', aspects: ['soak', String(i)] }] },
      (e) => events.push(e)
    );
    await node.ready();
    await node.attachTcpServer({ bind: '127.0.0.1:14270' });
    await node.announce(node.destinationHashes[0]);
    await node.stop();
    assert.ok(events.some((e) => e.type === 'nodeStopped'), `cycle ${i} missing nodeStopped`);
  }
});

test('auto interfaces attach and tear down without hardware', async () => {
  const node = startNode({}, () => {});
  try {
    await node.ready();
    const wifi = node.attachAutoWifi();
    const usb = node.attachAutoUsb({ baud: 115200 });
    const ble = node.attachAutoBle({ identitySecret: Buffer.alloc(16, 0x42) });
    await sleep(200);
    const interfaces = node.interfaces();
    assert.ok(interfaces.length >= 1);
    assert.equal(usb.kind ?? null, null);
    assert.equal(ble.kind, 'bluetooth-auto');
    assert.ok(ble.teardown());
    assert.ok(!ble.teardown());
    assert.ok(usb.teardown());
    assert.ok(wifi.teardown());
  } finally {
    await node.stop().catch(() => {});
  }
});

test('event overflow drops diagnostics and reports the gap', async () => {
  const clientEvents: AnyEvent[] = [];
  const server = startNode(
    {
      destinations: Array.from({ length: 65 }, (_, index) => ({
        appName: 'prnsnapi',
        aspects: ['overflow', String(index)],
      })),
    },
    () => {}
  );
  let blockFirstAnnounce = true;
  const client = startNode({ eventQueueLimit: 1 }, (e) => {
    clientEvents.push(e);
    if (blockFirstAnnounce && e.type === 'announce') {
      blockFirstAnnounce = false;
      const releaseAt = Date.now() + 400;
      while (Date.now() < releaseAt) {
        /* Keep the callback queue blocked while Rust emits diagnostics. */
      }
    }
  });
  try {
    await server.ready();
    const destinations = server.destinationHashes;
    await server.attachTcpServer({ bind: '127.0.0.1:14271' });
    await client.ready();
    await client.attachTcpClient({ target: '127.0.0.1:14271' });
    await sleep(500);
    const pending = destinations.slice(0, 64).map((destination) => server.announce(destination));
    await Promise.all(pending);
    await sleep(500);
    await server.announce(destinations[64]);
    await waitFor(
      () => clientEvents.some((e) => e.type === 'eventOverflow'),
      5000,
      'event overflow'
    );
    const overflow = clientEvents.find((e) => e.type === 'eventOverflow');
    assert.ok(overflow, 'missing eventOverflow after diagnostic shedding');
    assert.ok(overflow.droppedDiagnostics >= 1);
  } finally {
    await client.stop().catch(() => {});
    await server.stop().catch(() => {});
  }
});
