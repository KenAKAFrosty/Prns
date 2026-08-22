import { startNode } from '../../index.js';

const target = process.env.PRNS_TCP_TARGET;
if (!target) {
  console.error('FAILED PRNS_TCP_TARGET not set');
  process.exit(1);
}

const readyMessageType = 0x1339;
let phase = 'awaitingPrnsCloseDestination';
let firstDestination;
let secondLinkId;
let stopping = false;

const node = startNode({}, (event) => {
  if (event.type === 'announce') {
    const destination = Buffer.from(event.destination);
    if (phase === 'awaitingPrnsCloseDestination') {
      phase = 'closingFromPrns';
      firstDestination = destination;
      closeFromPrns(destination).catch(fail);
      return;
    }
    if (
      phase === 'awaitingStockCloseDestination' &&
      !destination.equals(firstDestination)
    ) {
      phase = 'awaitingStockClose';
      awaitStockClose(destination).catch(fail);
    }
    return;
  }
  if (event.type !== 'linkClosed' || secondLinkId === undefined) {
    return;
  }
  if (!Buffer.from(event.linkId).equals(secondLinkId) || event.reason !== 'peerClosed') {
    fail(
      new Error(
        `unexpected remote close link=${Buffer.from(event.linkId).toString('hex')} ` +
          `reason=${event.reason}`
      )
    );
    return;
  }
  console.log('NAPI_OBSERVED_STOCK_CLOSE reason=peerClosed');
});

async function closeFromPrns(destination) {
  const linkId = await node.establishLink(destination);
  const receipt = await node.sendChannelMessage(
    linkId,
    readyMessageType,
    Buffer.from('prns-ready-to-close')
  );
  requireProof(receipt);
  if (!node.closeLink(linkId)) {
    throw new Error('Prns did not queue the first Link close');
  }
  phase = 'awaitingStockCloseDestination';
  console.log('NAPI_CLOSED_STOCK_LINK queued=1');
}

async function awaitStockClose(destination) {
  secondLinkId = Buffer.from(await node.establishLink(destination));
  const receipt = await node.sendChannelMessage(
    secondLinkId,
    readyMessageType,
    Buffer.from('prns-ready-for-stock-close')
  );
  requireProof(receipt);
  console.log('NAPI_READY_FOR_STOCK_CLOSE proven=1');
}

function requireProof(receipt) {
  if (receipt.evidence !== 'proofExplicit' && receipt.evidence !== 'proofImplicit') {
    throw new Error(`readiness message settled without proof evidence=${receipt.evidence}`);
  }
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
await node.attachTcpClient({ target });
console.log('NAPI_LINK_CLOSURE_CLIENT_UP');

setTimeout(() => {
  fail(new Error(`timeout phase=${phase} secondLink=${secondLinkId !== undefined}`));
}, 35000);
