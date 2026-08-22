import { requestPathHash, startNode } from '../../index.js';

const role = process.env.PRNS_REJECTION_ROLE;
const target = process.env.PRNS_TCP_TARGET;
if (!role || !target) {
  console.error('FAILED PRNS_REJECTION_ROLE and PRNS_TCP_TARGET are required');
  process.exit(1);
}

const packedPrnsCompletion = Buffer.concat([
  Buffer.from([0xc4, Buffer.byteLength('prns-rejection-observed')]),
  Buffer.from('prns-rejection-observed'),
]);
const packedStockCompletion = Buffer.concat([
  Buffer.from([0xc4, Buffer.byteLength('stock-rejection-observed')]),
  Buffer.from('stock-rejection-observed'),
]);
let started = false;
let publishedResources = 0;
let stopping = false;
let rejectionPolicyReady = false;

const options =
  role === 'reject-stock'
    ? {
        destinations: [
          {
            appName: 'prns',
            aspects: ['resource', 'reject', 'napi'],
            maximumRequestBytes: 1024,
            requestPaths: [{ path: '/complete' }],
            linkRequests: 'acceptAll',
          },
        ],
      }
    : {};

const node = startNode(options, (event) => {
  if (event.type === 'resourceReceived' || event.type === 'resourceSegment') {
    publishedResources += 1;
    fail(new Error(`rejected Resource reached the application event stream type=${event.type}`));
    return;
  }
  if (role === 'send-to-stock' && event.type === 'announce' && !started) {
    started = true;
    sendToStock(event.destination).catch(fail);
    return;
  }
  if (role === 'reject-stock' && event.type === 'linkEstablished' && !rejectionPolicyReady) {
    rejectionPolicyReady = true;
    prepareStockRejection(event.linkId).catch(fail);
    return;
  }
  if (role === 'reject-stock' && event.type === 'request') {
    completeStockRejection(event).catch(fail);
  }
});

async function prepareStockRejection(linkId) {
  await node.setLinkResourceStrategy(linkId, { accept: 'if' });
  const receipt = await node.sendChannelMessage(
    linkId,
    0x1338,
    Buffer.from('prns-rejection-policy-ready')
  );
  if (receipt.evidence !== 'proofExplicit' && receipt.evidence !== 'proofImplicit') {
    throw new Error(`readiness message settled without proof evidence=${receipt.evidence}`);
  }
  console.log('NAPI_REJECTION_POLICY_READY');
}

async function sendToStock(destination) {
  const linkId = await node.establishLink(destination);
  let rejected = false;
  try {
    await node.sendResource(linkId, Buffer.alloc(128 * 1024, 0x5a), {
      compression: 'never',
    });
  } catch (error) {
    if (error.code !== 'PRNS_RESOURCE_REJECTED_BY_PEER') {
      throw error;
    }
    rejected = true;
  }
  if (!rejected) {
    throw new Error('stock receiver accepted the Resource configured for rejection');
  }
  const result = await node.request(linkId, requestPathHash('/complete'), packedPrnsCompletion, {
    timeoutMillis: 10000,
  });
  if (!Buffer.from(result.data).equals(Buffer.from('stock-no-publication'))) {
    throw new Error(`unexpected stock completion response ${Buffer.from(result.data).toString()}`);
  }
  console.log('NAPI_OBSERVED_STOCK_REJECTION published=0');
}

async function completeStockRejection(event) {
  if (stopping) {
    return;
  }
  if (
    !Buffer.from(event.pathHash).equals(requestPathHash('/complete')) ||
    !Buffer.from(event.data).equals(packedStockCompletion) ||
    publishedResources !== 0
  ) {
    throw new Error(
      `unexpected completion request path=${Buffer.from(event.pathHash).toString('hex')} ` +
        `data=${Buffer.from(event.data).toString('hex')} published=${publishedResources}`
    );
  }
  await node.respond(event.token, Buffer.from('prns-no-publication'));
  console.log('NAPI_REJECTED_STOCK published=0');
}

function fail(error) {
  if (stopping) {
    return;
  }
  stopping = true;
  console.error(`FAILED ${error.code ?? ''} ${error.message}`);
  process.exit(1);
}

await node.ready();
if (role === 'send-to-stock') {
  await node.attachTcpClient({ target });
  console.log('NAPI_REJECTION_CLIENT_UP');
} else if (role === 'reject-stock') {
  await node.attachTcpServer({ bind: target });
  const destination = node.destinationHashes[0];
  await node.announce(destination);
  setInterval(() => node.announce(destination).catch(fail), 500);
  console.log('NAPI_REJECTION_SERVER_UP');
} else {
  fail(new Error(`unknown rejection role ${role}`));
}

setTimeout(() => {
  fail(
    new Error(
      `timeout role=${role} started=${started} policyReady=${rejectionPolicyReady} ` +
        `published=${publishedResources}`
    )
  );
}, 35000);
