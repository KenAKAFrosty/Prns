import { startNode } from '../../index.js';

const local = process.env.PRNS_UDP_LOCAL;
const peer = process.env.PRNS_UDP_PEER;
if (!local || !peer) {
  console.error('FAILED PRNS_UDP_LOCAL and PRNS_UDP_PEER must be set');
  process.exit(1);
}

let ownDestination;
let announceTimer;
let sent = false;
let proven = false;
let received = false;
let stopping = false;

const node = startNode(
  {
    destinations: [
      {
        appName: 'prns',
        aspects: ['udp', 'client'],
        proof: 'proveAll',
      },
    ],
  },
  (event) => {
    if (event.type === 'announce' && !sent) {
      sent = true;
      node
        .sendSinglePacket(event.destination, Buffer.from('prns-udp-proof'))
        .then((receipt) => {
          requireProof(receipt);
          proven = true;
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
      Buffer.from(event.plaintext).toString() !== 'stock-udp-proof'
    ) {
      fail(new Error('unexpected stock UDP delivery'));
      return;
    }
    received = true;
    finishIfComplete().catch(fail);
  }
);

async function finishIfComplete() {
  if (stopping || !proven || !received) {
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
  console.log('NAPI_UDP_OK received=1 proven=1');
  process.exit(0);
}

function requireProof(receipt) {
  if (receipt.evidence !== 'proofExplicit' && receipt.evidence !== 'proofImplicit') {
    throw new Error(`UDP delivery settled without proof evidence=${receipt.evidence}`);
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
await node.attachUdp({ local, peer });
await node.announce(ownDestination);
announceTimer = setInterval(() => {
  node.announce(ownDestination).catch(fail);
}, 500);
console.log('UDP_CLIENT_UP');

setTimeout(() => {
  fail(new Error(`timeout sent=${sent} proven=${proven} received=${received}`));
}, 25000);
