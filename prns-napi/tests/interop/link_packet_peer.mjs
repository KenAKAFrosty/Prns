import { startNode } from '../../index.js';

const target = process.env.PRNS_TCP_TARGET;
if (!target) {
  console.error('FAILED PRNS_TCP_TARGET not set');
  process.exit(1);
}

const expectedFromStock = Buffer.from('stock-direct-link-packet');
const sentFromPrns = Buffer.from('prns-direct-link-packet');
let outboundStarted = false;
let outboundProven = false;
let receivedFromStock = false;
let stopping = false;

const node = startNode(
  {
    destinations: [
      {
        appName: 'prns',
        aspects: ['link', 'packet', 'napi'],
        proof: 'proveAll',
      },
    ],
  },
  (event) => {
    if (event.type === 'announce' && !outboundStarted) {
      outboundStarted = true;
      sendToStock(event.destination).catch(fail);
      return;
    }
    if (event.type !== 'linkDelivery') {
      return;
    }
    if (
      event.linkId.length !== 16 ||
      event.sourceInterface.length !== 8 ||
      !Buffer.from(event.plaintext).equals(expectedFromStock)
    ) {
      fail(
        new Error(
          `unexpected stock Link delivery link=${Buffer.from(event.linkId).toString('hex')} ` +
            `interface=${Buffer.from(event.sourceInterface).toString('hex')} ` +
            `plaintext=${Buffer.from(event.plaintext).toString('hex')}`
        )
      );
      return;
    }
    receivedFromStock = true;
    finishIfComplete().catch(fail);
  }
);

async function sendToStock(destination) {
  const linkId = await node.establishLink(destination);
  const receipt = await node.sendLinkPacket(linkId, sentFromPrns);
  if (receipt.evidence !== 'proofExplicit' && receipt.evidence !== 'proofImplicit') {
    throw new Error(`direct Link delivery settled without proof evidence=${receipt.evidence}`);
  }
  outboundProven = true;
  await finishIfComplete();
}

async function finishIfComplete() {
  if (stopping || !outboundProven || !receivedFromStock) {
    return;
  }
  stopping = true;
  await node.stop();
  console.log('NAPI_LINK_PACKET_OK received=1 proof=1');
  process.exit(0);
}

function fail(error) {
  if (stopping) {
    return;
  }
  stopping = true;
  console.error(`FAILED ${error.message}`);
  process.exit(1);
}

await node.ready();
await node.attachTcpClient({ target });
await node.announce(node.destinationHashes[0]);
const announceTimer = setInterval(() => {
  node.announce(node.destinationHashes[0]).catch(fail);
}, 500);
console.log('NAPI_LINK_PACKET_PEER_UP');

setTimeout(() => {
  clearInterval(announceTimer);
  fail(
    new Error(
      `timeout outboundStarted=${outboundStarted} outboundProven=${outboundProven} ` +
        `receivedFromStock=${receivedFromStock}`
    )
  );
}, 35000);
