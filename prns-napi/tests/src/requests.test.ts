import assert from 'node:assert/strict';
import { test } from 'node:test';

import { requestPathHash, startNode } from '../../index.js';
import { announceUntilHeard, bufEq, type AnyEvent } from './helpers.js';

test('link, request, respond, and unregistered-path refusal', async () => {
  const serverEvents: AnyEvent[] = [];
  const clientEvents: AnyEvent[] = [];

  const server = startNode(
    {
      destinations: [
        { appName: 'prnsnapi', aspects: ['reqsuite'], requestPaths: [{ path: '/echo' }] },
      ],
    },
    (e) => {
      serverEvents.push(e);
      if (e.type === 'request') {
        void server.respond(e.token, Buffer.concat([Buffer.from('echo:'), Buffer.from(e.data)]));
      }
    }
  );
  const client = startNode({}, (e) => clientEvents.push(e));
  try {
    await server.ready();
    const dest = server.destinationHashes[0];
    await server.attachTcpServer({ bind: '127.0.0.1:14262' });

    await client.ready();
    await client.attachTcpClient({ target: '127.0.0.1:14262' });
    await announceUntilHeard(server, dest, clientEvents, 'requests');

    const link = await client.establishLinkWithRtt(dest);
    assert.equal(link.linkId.length, 16);
    assert.ok(link.rttMillis >= 0);

    const echoHash = requestPathHash('/echo');
    const result = await client.request(link.linkId, echoHash, Buffer.from('ping'), {
      timeoutMillis: 5000,
    });
    assert.equal(Buffer.from(result.data).toString(), 'echo:ping');
    assert.ok(result.packed.length > result.data.length);

    const requestEvent = serverEvents.find((e) => e.type === 'request');
    assert.ok(requestEvent);
    assert.ok(bufEq(requestEvent.pathHash, echoHash));
    assert.ok(bufEq(requestEvent.destination, dest));

    await assert.rejects(
      () =>
        client.request(link.linkId, requestPathHash('/missing'), Buffer.from('x'), {
          timeoutMillis: 2000,
        }),
      (error: any) => error.code === 'PRNS_DELIVERY_TIMED_OUT'
    );

    assert.ok(client.closeLink(link.linkId));
  } finally {
    await client.stop().catch(() => {});
    await server.stop().catch(() => {});
  }
});
