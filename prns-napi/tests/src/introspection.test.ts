import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { test } from 'node:test';

import { startNode } from '../../index.js';
import { announceUntilHeard, bufEq, sleep, type AnyEvent } from './helpers.js';

test('introspection, routing control, and blackhole surfaces', async () => {
  const clientEvents: AnyEvent[] = [];
  const server = startNode(
    { destinations: [{ appName: 'prnsnapi', aspects: ['intro'] }] },
    () => {}
  );
  const client = startNode({}, (e) => clientEvents.push(e));
  try {
    await server.ready();
    const dest = server.destinationHashes[0];
    await server.attachTcpServer({ bind: '127.0.0.1:14269' });

    await client.ready();
    await client.attachTcpClient({ target: '127.0.0.1:14269' });
    await announceUntilHeard(server, dest, clientEvents, 'introspection');

    const interfaces = client.interfaces();
    assert.equal(interfaces.length, 1);
    assert.equal(interfaces[0].kind, 'tcp-client');
    assert.equal(interfaces[0].connection, 'connected');
    assert.ok(interfaces[0].rxBytes > 0n);

    const inventory = client.interfaceInventory();
    assert.equal(inventory.length, 1);
    assert.ok(inventory[0].origin.length > 0);
    assert.equal(inventory[0].interface.kind, 'tcp-client');

    const snapshot = await client.hostSnapshot();
    assert.equal(snapshot.backend.backend, 'Native');
    assert.ok(snapshot.backend.interfaceKinds.includes('TcpClient'));
    assert.equal(snapshot.interfaces.length, 1);
    assert.equal(snapshot.interfaces[0].kind, 'TcpClient');
    assert.equal(snapshot.runtime.interfaceCount, 1);

    const routes = await client.routes();
    assert.ok(routes.some((r: any) => bufEq(r.destination, dest)));
    const route = await client.route(dest);
    assert.ok(route);
    assert.equal(route.via ?? null, null);
    assert.equal(route.hops, 1);
    assert.equal(typeof route.learnedAtMillis, 'number');
    assert.equal(typeof route.lastRelayedAtMillis, 'number');
    assert.equal(typeof route.expiresAtMillis, 'number');

    const identityHash = await client.destinationIdentityHash(dest);
    assert.ok(identityHash);
    assert.equal(identityHash.length, 16);
    const identityInfo = await client.destinationIdentity({ destination: dest });
    assert.ok(identityInfo);
    assert.ok(bufEq(identityInfo.identity, identityHash));
    assert.equal(identityInfo.publicKey.length, 64);

    const linkId = await client.establishLink(dest);
    await sleep(100);
    assert.equal(await client.linkCount(), 1);
    client.closeLink(linkId);

    await client.announceRates();

    const dropped = await client.dropRoute(dest);
    assert.equal(dropped, true);
    assert.equal((await client.route(dest)) ?? null, null);
    assert.equal(await client.dropRoute(dest), false);

    const cleared = await client.clearAnnounceQueues();
    assert.ok(cleared >= 0);

    const stranger = randomBytes(16);
    assert.deepEqual(await client.blackholedIdentities(), []);
    assert.equal(await client.blackholeIdentity(stranger, 'test entry'), 'added');
    assert.equal(await client.blackholeIdentity(stranger), 'alreadyPresent');
    assert.equal(await client.isBlackholed(stranger), true);
    const listed = await client.blackholedIdentities();
    assert.equal(listed.length, 1);
    assert.ok(bufEq(listed[0].identity, stranger));
    assert.equal(listed[0].reason, 'test entry');
    assert.equal(listed[0].indefinite, true);
    assert.equal(await client.unblackholeIdentity(stranger), 'removed');
    assert.equal(await client.isBlackholed(stranger), false);

    const marked = await client.markDestinationUsed(dest);
    assert.ok(typeof marked === 'string');
  } finally {
    await client.stop().catch(() => {});
    await server.stop().catch(() => {});
  }
});
