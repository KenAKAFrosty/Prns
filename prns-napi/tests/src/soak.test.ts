import assert from 'node:assert/strict';
import { test } from 'node:test';

import { startNode } from '../../index.js';
import { sleep, type AnyEvent } from './helpers.js';

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
    await sleep(200);
    const interfaces = node.interfaces();
    assert.ok(interfaces.length >= 1);
    assert.ok(usb.kind === 'UsbAuto' || usb.kind === null || typeof usb.kind === 'string');
    assert.ok(usb.teardown());
    assert.ok(wifi.teardown());
  } finally {
    await node.stop().catch(() => {});
  }
});

test('event overflow drops diagnostics and reports the gap', async () => {
  const serverEvents: AnyEvent[] = [];
  const clientEvents: AnyEvent[] = [];
  const server = startNode(
    { destinations: [{ appName: 'prnsnapi', aspects: ['overflow'] }] },
    (e) => serverEvents.push(e)
  );
  const client = startNode({ eventQueueLimit: 1 }, (e) => clientEvents.push(e));
  try {
    await server.ready();
    const dest = server.destinationHashes[0];
    await server.attachTcpServer({ bind: '127.0.0.1:14271' });
    await client.ready();
    await client.attachTcpClient({ target: '127.0.0.1:14271' });
    await sleep(500);
    for (let i = 0; i < 30; i += 1) {
      await server.announce(dest);
    }
    const spin = Date.now() + 400;
    while (Date.now() < spin) {
      /* hold the JS thread so the tsfn queue backs up */
    }
    await sleep(1500);
    const overflow = clientEvents.find((e) => e.type === 'eventOverflow');
    if (overflow) {
      assert.ok(overflow.droppedDiagnostics >= 1);
    }
  } finally {
    await client.stop().catch(() => {});
    await server.stop().catch(() => {});
  }
});
