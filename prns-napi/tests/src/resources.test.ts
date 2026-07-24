import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';

import { startNode } from '../../index.js';
import { announceUntilHeard, waitFor, type AnyEvent } from './helpers.js';

test('resource quantity aliases reject conflicting values', () => {
  assert.throws(
    () =>
      startNode(
        {
          destinations: [
            {
              appName: 'prnsnapi',
              aspects: ['resalias'],
              resourceStrategy: {
                accept: 'all',
                maxUncompressedBytes: 4_096,
                maxUncompressedLen: 8_192,
              },
            },
          ],
        },
        () => {}
      ),
    (error: any) => error.code === 'PRNS_INVALID_ARGUMENT'
  );
});

test('resource transfer with metadata and progress events', async () => {
  const serverEvents: AnyEvent[] = [];
  const clientEvents: AnyEvent[] = [];

  const server = startNode(
    {
      destinations: [
        {
          appName: 'prnsnapi',
          aspects: ['res'],
          resourceStrategy: { accept: 'all', maxUncompressedBytes: 1_000_000 },
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
    assert.equal(received.totalSizeBytes, payload.length);
    assert.equal(received.totalSize, received.totalSizeBytes);
    await waitFor(
      () => clientEvents.some((e) => e.type === 'resourceSendProgress'),
      3000,
      'progress event'
    );
    const progress = clientEvents.find((e) => e.type === 'resourceSendProgress');
    assert.ok(progress);
    assert.equal(progress.transferredBytes, progress.totalBytes);
    assert.ok(progress.physicalTransferredBytes > 0);
    assert.equal(progress.transferred, progress.transferredBytes);
    assert.equal(progress.total, progress.totalBytes);
    assert.equal(progress.physicalTransferred, progress.physicalTransferredBytes);

    const sourcePath = path.join(os.tmpdir(), `prns-napi-res-src-${process.pid}`);
    const sinkPath = path.join(os.tmpdir(), `prns-napi-res-sink-${process.pid}`);
    const filePayload = randomBytes(64_000);
    fs.writeFileSync(sourcePath, filePayload);
    try {
      const fileReceiving = server.receiveResourceFile(linkId, sinkPath);
      await client.sendResourceFile(linkId, sourcePath);
      const fileReceipt = await fileReceiving;
      assert.equal(Buffer.compare(fs.readFileSync(sinkPath), filePayload), 0);
      assert.ok(fileReceipt.totalSizeBytes >= filePayload.length);
      assert.equal(fileReceipt.totalSize, fileReceipt.totalSizeBytes);
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
      (error: any) => error.code === 'PRNS_DELIVERY_TIMED_OUT'
    );
  } finally {
    await client.stop().catch(() => {});
    await server.stop().catch(() => {});
  }
});
