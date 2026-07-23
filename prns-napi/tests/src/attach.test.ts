import assert from 'node:assert/strict';
import { test } from 'node:test';

import { startNode, type PrnsNode } from '../../index.js';
import { announceUntilHeard, type AnyEvent } from './helpers.js';

async function announcePair(
  label: string,
  attachA: (node: PrnsNode) => Promise<void>,
  attachB: (node: PrnsNode) => Promise<void>
): Promise<void> {
  const eventsB: AnyEvent[] = [];
  const a = startNode(
    { destinations: [{ appName: 'prnsnapi', aspects: ['attach', label] }] },
    () => {}
  );
  const b = startNode({}, (e) => eventsB.push(e));
  try {
    await a.ready();
    await b.ready();
    const dest = a.destinationHashes[0];
    await attachA(a);
    await attachB(b);
    await announceUntilHeard(a, dest, eventsB, label);
  } finally {
    await b.stop().catch(() => {});
    await a.stop().catch(() => {});
  }
}

test('attachConfig stands up a TCPServerInterface from RNS config text', async () => {
  await announcePair(
    'configTcp',
    async (node) => {
      const configText = [
        '[reticulum]',
        '  enable_transport = False',
        '',
        '[interfaces]',
        '  [[Config TCP Server]]',
        '    type = TCPServerInterface',
        '    interface_enabled = True',
        '    listen_ip = 127.0.0.1',
        '    listen_port = 14263',
      ].join('\n');
      const result = await node.attachConfig(configText);
      assert.equal(result.failures.length, 0);
      assert.equal(result.attached.length, 1);
      assert.equal(result.attached[0].name, 'Config TCP Server');
    },
    async (node) => {
      await node.attachTcpClient({ target: '127.0.0.1:14263' });
    }
  );
});

test('attachConfig rejects malformed config with PRNS_CONFIG_INVALID', async () => {
  const node = startNode({}, () => {});
  try {
    await node.ready();
    await assert.rejects(
      () => node.attachConfig('[interfaces\n  broken ='),
      (error: any) => error.code === 'PRNS_CONFIG_INVALID'
    );
    const skipped = await node.attachConfig(
      '[interfaces]\n  [[Broken]]\n    type = NoSuchInterface'
    );
    assert.equal(skipped.attached.length, 0);
    assert.equal(skipped.failures.length, 0);
  } finally {
    await node.stop().catch(() => {});
  }
});

test('udp pair exchanges announces', async () => {
  await announcePair(
    'udp',
    async (node) => {
      await node.attachUdp({ local: '127.0.0.1:14264', peer: '127.0.0.1:14265' });
    },
    async (node) => {
      await node.attachUdp({ local: '127.0.0.1:14265', peer: '127.0.0.1:14264' });
    }
  );
});

test('shared instance server and local client exchange announces', async () => {
  await announcePair(
    'sharedInstance',
    async (node) => {
      await node.attachSharedInstanceServer({ port: 14266 });
    },
    async (node) => {
      await node.attachSharedInstanceClient({ port: 14266 });
    }
  );
});
