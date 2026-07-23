import assert from 'node:assert/strict';
import { test } from 'node:test';

import { startNode } from '../../index.js';
import { announceUntilHeard, bufEq, waitFor, type AnyEvent } from './helpers.js';

test('two nodes exchange a proven single packet over TCP', async () => {
  const serverEvents: AnyEvent[] = [];
  const clientEvents: AnyEvent[] = [];

  const server = startNode(
    { destinations: [{ appName: 'prnsnapi', aspects: ['loop', 'server'] }] },
    (e) => serverEvents.push(e)
  );
  const client = startNode({}, (e) => clientEvents.push(e));
  try {
    await server.ready();
    const dest = server.destinationHashes[0];
    const listener = await server.attachTcpServer({ bind: '127.0.0.1:14261' });
    assert.equal(listener.kind, 'TcpServer');
    assert.equal(listener.id.length, 8);

    await client.ready();
    const dialer = await client.attachTcpClient({ target: '127.0.0.1:14261' });
    assert.equal(dialer.kind, 'TcpClient');

    await announceUntilHeard(server, dest, clientEvents, 'loopback');

    const payload = Buffer.from('loopback payload');
    const receipt = await client.sendSinglePacket(dest, payload);
    assert.match(receipt.evidence, /^proof(Explicit|Implicit)$/);
    assert.ok(receipt.rttMillis >= 0);

    await waitFor(
      () => serverEvents.some((e) => e.type === 'singleDelivery' && bufEq(e.plaintext, payload)),
      5000,
      'delivery event'
    );

    assert.ok(dialer.teardown());
    assert.ok(!dialer.teardown());
  } finally {
    await client.stop().catch(() => {});
    await server.stop().catch(() => {});
  }
  assert.ok(clientEvents.some((e) => e.type === 'nodeStopped'));
  assert.ok(serverEvents.some((e) => e.type === 'nodeStopped'));
});
