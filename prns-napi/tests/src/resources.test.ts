import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';

import { startNode } from '../../index.js';
import { announceUntilHeard, waitFor, type AnyEvent } from './helpers.js';

test('resource transfer with metadata and progress events', async () => {
  const serverEvents: AnyEvent[] = [];
  const clientEvents: AnyEvent[] = [];

  const server = startNode(
    {
      destinations: [
        {
          appName: 'prnsnapi',
          aspects: ['res'],
          resourceStrategy: { accept: 'all' },
        },
      ],
    },
    (e) => serverEvents.push(e)
  );
  const client = startNode({}, (e) => clientEvents.push(e));
  try {
    await server.ready();
    const dest = server.destinationHashes[0];
    await server.attachTcpServer({ bind: '127.0.0.1:14267' });

    await client.ready();
    await client.attachTcpClient({ target: '127.0.0.1:14267' });
    await announceUntilHeard(server, dest, clientEvents, 'resources');

    const linkId = await client.establishLink(dest);
    await waitFor(
      () => serverEvents.some((e) => e.type === 'linkEstablished'),
      5000,
      'server link event'
    );

    const payload = randomBytes(200_000);
    const metadata = Buffer.from('resource metadata');
    const receiving = server.receiveResource(linkId);
    await client.sendResource(linkId, payload, { metadata, progress: true });
    const received = await receiving;

    assert.equal(Buffer.compare(Buffer.from(received.data), payload), 0);
    assert.ok(received.metadata);
    assert.equal(Buffer.compare(Buffer.from(received.metadata), metadata), 0);
    assert.equal(received.totalSize, payload.length);
    await waitFor(
      () => clientEvents.some((e) => e.type === 'resourceSendProgress'),
      3000,
      'progress event'
    );

    const sourcePath = path.join(os.tmpdir(), `prns-napi-res-src-${process.pid}`);
    const sinkPath = path.join(os.tmpdir(), `prns-napi-res-sink-${process.pid}`);
    const filePayload = randomBytes(64_000);
    fs.writeFileSync(sourcePath, filePayload);
    try {
      const fileReceiving = server.receiveResourceFile(linkId, sinkPath);
      await client.sendResourceFile(linkId, sourcePath);
      const fileReceipt = await fileReceiving;
      assert.equal(Buffer.compare(fs.readFileSync(sinkPath), filePayload), 0);
      assert.ok(fileReceipt.totalSize >= filePayload.length);
    } finally {
      fs.rmSync(sourcePath, { force: true });
      fs.rmSync(sinkPath, { force: true });
    }
  } finally {
    await client.stop().catch(() => {});
    await server.stop().catch(() => {});
  }
});

test('resources are refused when the strategy stays acceptNone', async () => {
  const serverEvents: AnyEvent[] = [];
  const clientEvents: AnyEvent[] = [];
  const server = startNode(
    { destinations: [{ appName: 'prnsnapi', aspects: ['resnone'] }] },
    (e) => serverEvents.push(e)
  );
  const client = startNode({}, (e) => clientEvents.push(e));
  try {
    await server.ready();
    const dest = server.destinationHashes[0];
    await server.attachTcpServer({ bind: '127.0.0.1:14268' });
    await client.ready();
    await client.attachTcpClient({ target: '127.0.0.1:14268' });
    await announceUntilHeard(server, dest, clientEvents, 'resnone');

    const linkId = await client.establishLink(dest);
    await assert.rejects(
      () => client.sendResource(linkId, Buffer.from('refused payload')),
      (error: any) => error.code === 'PRNS_RESOURCE_SEND_FAILED'
    );
  } finally {
    await client.stop().catch(() => {});
    await server.stop().catch(() => {});
  }
});
