import { startNode } from '../../index.js';

const target = process.env.PRNS_TCP_TARGET;
if (!target) {
  console.error('FAILED PRNS_TCP_TARGET not set');
  process.exit(1);
}

const messageType = 0x1337;
const expected = ['stock-channel-zero', 'stock-channel-one'];
const received = [];
let started = false;
let outboundDelivered = false;
let stopping = false;

const node = startNode({}, (event) => {
  if (event.type === 'announce' && !started) {
    started = true;
    exchange(event.destination).catch(fail);
    return;
  }
  if (event.type !== 'channelMessage') {
    return;
  }
  const payload = Buffer.from(event.data).toString();
  const expectedPayload = expected[received.length];
  if (event.messageType !== messageType || payload !== expectedPayload) {
    fail(new Error(`unexpected channel message type=${event.messageType} payload=${payload}`));
    return;
  }
  received.push(payload);
  finishIfComplete().catch(fail);
});

async function exchange(destination) {
  const linkId = await node.establishLink(destination);
  const firstReceipt = await node.sendChannelMessage(
    linkId,
    messageType,
    Buffer.from('prns-channel-zero')
  );
  requireProof(firstReceipt);
  const secondReceipt = await node.sendChannelMessage(
    linkId,
    messageType,
    Buffer.from('prns-channel-one')
  );
  requireProof(secondReceipt);
  outboundDelivered = true;
  await finishIfComplete();
}

async function finishIfComplete() {
  if (stopping || !outboundDelivered || received.length !== expected.length) {
    return;
  }
  stopping = true;
  try {
    await node.stop();
  } catch (error) {
    console.error(`FAILED ${error.message}`);
    process.exit(1);
  }
  console.log('NAPI_CHANNEL_OK messages=2 ordered=1 proven=2');
  process.exit(0);
}

function requireProof(receipt) {
  if (receipt.evidence !== 'proofExplicit' && receipt.evidence !== 'proofImplicit') {
    throw new Error(`channel delivery settled without proof evidence=${receipt.evidence}`);
  }
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
console.log('CHANNEL_CLIENT_UP');

setTimeout(() => {
  fail(new Error(`timeout started=${started} outbound=${outboundDelivered} received=${received.length}`));
}, 30000);
