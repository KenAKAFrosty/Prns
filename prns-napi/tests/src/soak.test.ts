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
    const ble = node.attachAutoBluetoothLe({ identitySecret: Buffer.alloc(16, 0x42) });
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
  const events: AnyEvent[] = [];
  let blockFirstAnnounce = true;
  const node = startNode({ eventQueueLimit: 1 }, (event) => {
    events.push(event);
    if (blockFirstAnnounce && event.type === 'announce') {
      blockFirstAnnounce = false;
      const releaseAt = Date.now() + 1_500;
      while (Date.now() < releaseAt) {
        /* Keep JavaScript blocked while independent peers emit diagnostics. */
      }
    }
  });
  const peers = Array.from({ length: 16 }, (_, index) =>
    startNode(
      {
        destinations: [{ appName: 'prnsnapi', aspects: ['overflow', String(index)] }],
      },
      () => {}
    )
  );
  try {
    await node.ready();
    await node.attachTcpServer({ bind: '127.0.0.1:14271' });
    await Promise.all(
      peers.map(async (peer) => {
        await peer.ready();
        await peer.attachTcpClient({ target: '127.0.0.1:14271' });
      })
    );
    await sleep(500);
    await Promise.all(peers.map((peer) => peer.announce(peer.destinationHashes[0])));
    await sleep(500);
    await node.stop();
    await waitFor(
      () => events.some((event) => event.type === 'eventOverflow'),
      5000,
      'event overflow'
    ).catch((error: Error) => {
      const counts = Object.entries(
        events.reduce<Record<string, number>>((byType, event) => {
          byType[event.type] = (byType[event.type] ?? 0) + 1;
          return byType;
        }, {})
      );
      throw new Error(`${error.message}; received ${JSON.stringify(Object.fromEntries(counts))}`);
    });
    const overflow = events.find((event) => event.type === 'eventOverflow');
    assert.ok(overflow, 'missing eventOverflow after diagnostic shedding');
    assert.ok(overflow.droppedDiagnostics >= 1);
  } finally {
    await node.stop().catch(() => {});
    await Promise.all(peers.map((peer) => peer.stop().catch(() => {})));
  }
});
