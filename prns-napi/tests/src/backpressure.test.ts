import assert from 'node:assert/strict';
import { test } from 'node:test';

import { startNode } from '../../index.js';
import { announceUntilHeard, waitFor, type AnyEvent } from './helpers.js';

test('application byte pressure terminates explicitly', async () => {
  const serverEvents: AnyEvent[] = [];
  const clientEvents: AnyEvent[] = [];
  const server = startNode(
    {
      destinations: [{ appName: 'prnsnapi', aspects: ['backpressure', 'server'] }],
      applicationEventQueueLimit: 1,
      retainedEventBytesLimit: 1,
      diagnosticEventQueueLimit: 8,
    },
    (event) => serverEvents.push(event)
  );
  const client = startNode({}, (event) => clientEvents.push(event));
  try {
    await server.ready();
    const destination = server.destinationHashes[0];
    await server.attachTcpServer({ bind: '127.0.0.1:14272' });
    await client.ready();
    await client.attachTcpClient({ target: '127.0.0.1:14272' });
    await announceUntilHeard(server, destination, clientEvents, 'backpressure');
    const sending = client
      .sendSinglePacket(destination, Buffer.from([1, 2]))
      .catch(() => undefined);
    await waitFor(
      () => serverEvents.some((event) => event.type === 'eventBackpressureExceeded'),
      5_000,
      'event backpressure failure'
    );
    const failure = serverEvents.find(
      (event) => event.type === 'eventBackpressureExceeded'
    );
    assert.equal(failure?.rejectedEventBytes, 2);
    assert.ok(!serverEvents.some((event) => event.type === 'singleDelivery'));
    void sending;
  } finally {
    await client.stop().catch(() => {});
    await server.stop().catch(() => {});
  }
});
