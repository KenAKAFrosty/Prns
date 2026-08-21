import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';

import { startNode, type InterfaceConfigSpec } from '../../index.js';

type FixtureInterface = InterfaceConfigSpec & {
  bitrate?: { kind: 'Auto' | 'BitsPerSecond'; value?: number };
};

type InterfaceFixture = {
  schemaVersion: number;
  interfaces: FixtureInterface[];
};

test('typed interface attachment uses the canonical host and preserves failure kinds', async () => {
  const node = startNode({}, () => {});
  try {
    const tcp = await node.attachInterface({
      kind: 'TcpClient',
      target: '127.0.0.1:9'
    });
    assert.equal(tcp.kind, 'TcpClient');
    assert.equal(tcp.id.length, 8);
    assert.equal(tcp.teardown(), true);

    await assert.rejects(
      node.attachInterface({
        kind: 'BrowserRendezvous',
        url: 'ws://fixture.invalid/rendezvous'
      }),
      (error: unknown) => errorCode(error) === 'PRNS_UNSUPPORTED'
    );

    await assert.rejects(
      node.attachInterface({
        kind: 'Serial',
        port: '/dev/tty-fixture',
        line: {
          baud: 115200,
          dataBits: 'Nine',
          parity: 'None',
          stopBits: 'One'
        }
      }),
      (error: unknown) => errorCode(error) === 'PRNS_CONFIG_INVALID'
    );
  } finally {
    await node.stop().catch(() => {});
  }
});

test('every shared typed interface fixture marshals without touching hardware', async () => {
  const fixture = JSON.parse(
    readFileSync(
      resolve(process.cwd(), '../prns-host/conformance/interface-configs-v1.json'),
      'utf8'
    )
  ) as InterfaceFixture;
  assert.equal(fixture.schemaVersion, 1);
  assert.equal(fixture.interfaces.length, 19);

  const node = startNode({}, () => {});
  try {
    const kinds = fixture.interfaces.map((config) => {
      const { bitrate, ...raw } = config;
      const marshalled: InterfaceConfigSpec =
        bitrate?.kind === 'BitsPerSecond'
          ? { ...raw, bitrateBps: bitrate.value }
          : raw;
      return node.previewValidateInterfaceConfig(marshalled);
    });
    assert.deepEqual(
      kinds,
      fixture.interfaces.map((config) => config.kind)
    );
  } finally {
    await node.stop().catch(() => {});
  }
});

test('websocket framing selection is required and closed before attachment', async () => {
  const node = startNode({}, () => {});
  try {
    assert.equal(
      node.previewValidateInterfaceConfig({
        kind: 'WebSocketClient',
        target: 'ws://fixture.invalid/client',
        framing: 'Auto'
      }),
      'WebSocketClient'
    );
    assert.throws(
      () =>
        node.previewValidateInterfaceConfig({
          kind: 'WebSocketClient',
          target: 'ws://fixture.invalid/client'
        }),
      (error: unknown) => errorCode(error) === 'PRNS_CONFIG_INVALID'
    );
    assert.throws(
      () =>
        node.previewValidateInterfaceConfig({
          kind: 'WebSocketServer',
          bind: '127.0.0.1:4242',
          framing: 'HDLC' as never
        }),
      (error: unknown) => errorCode(error) === 'PRNS_CONFIG_INVALID'
    );
  } finally {
    await node.stop().catch(() => {});
  }
});

function errorCode(error: unknown): string | undefined {
  if (error instanceof Error && 'code' in error && typeof error.code === 'string') {
    return error.code;
  }
  return undefined;
}
