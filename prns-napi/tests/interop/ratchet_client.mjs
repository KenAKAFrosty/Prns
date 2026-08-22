import { startNode } from '../../index.js';

const target = process.env.PRNS_TCP_TARGET;
if (!target) {
  console.error('FAILED PRNS_TCP_TARGET not set');
  process.exit(1);
}

const sentPayloads = ['prns-ratchet-zero', 'prns-ratchet-one'];
let ownDestination;
let remoteAnnounces = 0;
let deliveredToStock = 0;
let receivedFromStock = false;
let stopping = false;
let announceTimer;

const node = startNode(
  {
    destinations: [
      {
        appName: 'prns',
        aspects: ['ratchet', 'client'],
        proof: 'proveAll',
        ratchet: 'ratchetsRequired',
      },
    ],
  },
  (event) => {
    if (event.type === 'announce') {
      if (announceTimer !== undefined) {
        clearInterval(announceTimer);
        announceTimer = undefined;
      }
      if (remoteAnnounces >= sentPayloads.length) {
        fail(new Error(`unexpected extra stock ratchet announce ${remoteAnnounces + 1}`));
        return;
      }
      const payload = sentPayloads[remoteAnnounces];
      remoteAnnounces += 1;
      node
        .sendSinglePacket(event.destination, Buffer.from(payload))
        .then((receipt) => {
          requireProof(receipt);
          deliveredToStock += 1;
          return finishIfComplete();
        })
        .catch(fail);
      return;
    }
    if (event.type !== 'singleDelivery') {
      return;
    }
    if (
      !Buffer.from(event.destination).equals(ownDestination) ||
      Buffer.from(event.plaintext).toString() !== 'stock-ratchet-proof'
    ) {
      fail(new Error('unexpected stock ratchet delivery'));
      return;
    }
    receivedFromStock = true;
    finishIfComplete().catch(fail);
  }
);

async function finishIfComplete() {
  if (
    stopping ||
    remoteAnnounces !== sentPayloads.length ||
    deliveredToStock !== sentPayloads.length ||
    !receivedFromStock
  ) {
    return;
  }
  stopping = true;
  if (announceTimer !== undefined) {
    clearInterval(announceTimer);
  }
  try {
    await node.stop();
  } catch (error) {
    console.error(`FAILED ${error.message}`);
    process.exit(1);
  }
  console.log('NAPI_RATCHET_OK sent=2 received=1 proven=2');
  process.exit(0);
}

function requireProof(receipt) {
  if (receipt.evidence !== 'proofExplicit' && receipt.evidence !== 'proofImplicit') {
    throw new Error(`ratchet delivery settled without proof evidence=${receipt.evidence}`);
  }
}

function fail(error) {
  if (stopping) {
    return;
  }
  stopping = true;
  if (announceTimer !== undefined) {
    clearInterval(announceTimer);
  }
  console.error(`FAILED ${error.message}`);
  process.exit(1);
}

await node.ready();
ownDestination = Buffer.from(node.destinationHashes[0]);
await node.attachTcpClient({ target });
await node.announce(ownDestination);
announceTimer = setInterval(() => {
  node.announce(ownDestination).catch(fail);
}, 500);
console.log('RATCHET_CLIENT_UP');

setTimeout(() => {
  fail(
    new Error(
      `timeout announces=${remoteAnnounces} delivered=${deliveredToStock} received=${receivedFromStock}`
    )
  );
}, 30000);
