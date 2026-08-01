import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import path from 'node:path';
import { test } from 'node:test';
import { promisify } from 'node:util';

import { generateIdentitySecret, requestPathHash, startNode, version } from '../../index.js';

const run = promisify(execFile);

test('module loads and reports the workspace version', () => {
  assert.match(version(), /^\d+\.\d+\.\d+$/);
});

test('identity secrets are 64 bytes and unique', () => {
  const a = generateIdentitySecret();
  const b = generateIdentitySecret();
  assert.equal(a.length, 64);
  assert.equal(b.length, 64);
  assert.notEqual(Buffer.compare(a, b), 0);
});

test('request path hashes are stable 16-byte truncated sha256', () => {
  const a = requestPathHash('/echo');
  const b = requestPathHash('/echo');
  const c = requestPathHash('/other');
  assert.equal(a.length, 16);
  assert.equal(Buffer.compare(a, b), 0);
  assert.notEqual(Buffer.compare(a, c), 0);
});

test('destination hashes are deterministic for a fixed identity', async () => {
  const identity = Buffer.alloc(64, 0x42);
  const make = () =>
    startNode(
      { destinations: [{ appName: 'prnsnapi', aspects: ['smoke'], identity: { secret: identity } }] },
      () => {}
    );
  const first = make();
  const second = make();
  try {
    assert.equal(first.identityHash.length, 16);
    assert.equal(second.identityHash.length, 16);
    assert.equal(Buffer.compare(first.destinationHashes[0], second.destinationHashes[0]), 0);
    await first.ready();
    await second.ready();
  } finally {
    await first.stop().catch(() => {});
    await second.stop().catch(() => {});
  }
});

test('invalid arguments throw synchronously with PRNS codes', () => {
  assert.throws(
    () =>
      startNode(
        { destinations: [{ appName: 'x', aspects: [], kind: 'nonsense' as any }] },
        () => {}
      ),
    (error: any) => error.code === 'PRNS_INVALID_ARGUMENT'
  );
  assert.throws(
    () =>
      startNode(
        { destinations: [{ appName: 'x', aspects: [], identity: { secret: Buffer.alloc(3) } }] },
        () => {}
      ),
    (error: any) => error.code === 'PRNS_INVALID_ARGUMENT'
  );
});

test('async rejections carry PRNS codes after stop', async () => {
  const node = startNode({ destinations: [{ appName: 'prnsnapi', aspects: ['codes'] }] }, () => {});
  const dest = node.destinationHashes[0];
  try {
    await node.ready();
  } finally {
    await node.stop().catch(() => {});
  }
  await assert.rejects(
    () => node.announce(dest),
    (error: any) => error.code === 'PRNS_NODE_STOPPED'
  );
});

test('stop is idempotent', async () => {
  const node = startNode({}, () => {});
  await node.ready();
  await node.stop();
  await node.stop();
});

test('process exits promptly once nodes are stopped', async () => {
  const fixture = path.join(__dirname, '..', 'fixtures', 'exit.js');
  const { stdout } = await run(process.execPath, [fixture], { timeout: 20000 });
  assert.match(stdout, /fixture-exit-ok/);
});
