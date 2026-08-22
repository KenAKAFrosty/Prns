import { createHash } from 'node:crypto';

import { requestPathHash, startNode } from '../../index.js';

const target = process.env.PRNS_TCP_TARGET;
if (!target) {
  console.error('FAILED PRNS_TCP_TARGET not set');
  process.exit(1);
}

const responseSize = 128 * 1024;

function deterministicPayload(seed) {
  const blocks = [];
  let generated = 0;
  let counter = 0;
  while (generated < responseSize) {
    const suffix = Buffer.alloc(8);
    suffix.writeBigUInt64BE(BigInt(counter));
    const block = createHash('sha256').update(seed).update(suffix).digest();
    blocks.push(block);
    generated += block.length;
    counter += 1;
  }
  return Buffer.concat(blocks).subarray(0, responseSize);
}

const expectedStockResponse = deterministicPayload(Buffer.from('stock-large-response'));
const prnsResponse = deterministicPayload(Buffer.from('prns-large-response'));
const packedStockRequest = Buffer.concat([
  Buffer.from([0xc4, Buffer.byteLength('stock-request')]),
  Buffer.from('stock-request'),
]);
const packedPrnsRequest = Buffer.concat([
  Buffer.from([0xc4, Buffer.byteLength('prns-request')]),
  Buffer.from('prns-request'),
]);
let ownDestination;
let announceTimer;
let requestedStock = false;
let stockResponseReceived = false;
let respondedToStock = false;
let stopping = false;

const node = startNode(
  {
    destinations: [
      {
        appName: 'prns',
        aspects: ['large', 'client'],
        maximumRequestBytes: 1024,
        requestPaths: [{ path: '/large' }],
        linkRequests: 'acceptAll',
      },
    ],
  },
  (event) => {
    if (event.type === 'announce' && !requestedStock) {
      requestedStock = true;
      if (announceTimer !== undefined) {
        clearInterval(announceTimer);
        announceTimer = undefined;
      }
      requestStock(event.destination).catch(fail);
      return;
    }
    if (event.type !== 'request') {
      return;
    }
    if (
      !Buffer.from(event.destination).equals(ownDestination) ||
      !Buffer.from(event.pathHash).equals(requestPathHash('/large')) ||
      !Buffer.from(event.data).equals(packedStockRequest)
    ) {
      fail(
        new Error(
          `unexpected stock large request destination=${Buffer.from(event.destination).toString('hex')} ` +
            `expectedDestination=${ownDestination.toString('hex')} ` +
            `path=${Buffer.from(event.pathHash).toString('hex')} ` +
            `expectedPath=${requestPathHash('/large').toString('hex')} ` +
            `data=${Buffer.from(event.data).toString('hex')}`
        )
      );
      return;
    }
    node
      .respond(event.token, prnsResponse)
      .then(() => {
        respondedToStock = true;
        return finishIfComplete();
      })
      .catch(fail);
  }
);

async function requestStock(destination) {
  const linkId = await node.establishLink(destination);
  const result = await node.request(
    linkId,
    requestPathHash('/large'),
    packedPrnsRequest,
    { timeoutMillis: 30000, maximumResponseBytes: responseSize + 1024 }
  );
  if (!Buffer.from(result.data).equals(expectedStockResponse)) {
    throw new Error(`unexpected stock response length=${result.data.length}`);
  }
  stockResponseReceived = true;
  await finishIfComplete();
}

async function finishIfComplete() {
  if (stopping || !stockResponseReceived || !respondedToStock) {
    return;
  }
  stopping = true;
  try {
    await node.stop();
  } catch (error) {
    console.error(`FAILED ${error.message}`);
    process.exit(1);
  }
  console.log(`NAPI_LARGE_REQUEST_OK response=${responseSize} responded=${responseSize}`);
  process.exit(0);
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
console.log('LARGE_REQUEST_CLIENT_UP');

setTimeout(() => {
  fail(
    new Error(
      `timeout requested=${requestedStock} stockResponse=${stockResponseReceived} responded=${respondedToStock}`
    )
  );
}, 35000);
