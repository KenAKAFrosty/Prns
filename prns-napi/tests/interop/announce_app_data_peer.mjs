import { startNode } from '../../index.js';

const target = process.env.PRNS_TCP_TARGET;
if (!target) {
  console.error('FAILED PRNS_TCP_TARGET not set');
  process.exit(1);
}

const expectedFromStock = Buffer.from([0xff, 0x73, 0x74, 0x6f, 0x63, 0x6b, 0x00]);
const sentFromPrns = Buffer.from([0x00, 0x70, 0x72, 0x6e, 0x73, 0xff]);
let receivedFromStock = false;
let stopping = false;

const node = startNode(
  {
    destinations: [
      {
        appName: 'prns',
        aspects: ['announce', 'appdata', 'napi'],
        announceAppData: sentFromPrns,
      },
    ],
  },
  (event) => {
    if (event.type !== 'announce' || !Buffer.from(event.appData).equals(expectedFromStock)) {
      return;
    }
    if (event.destination.length !== 16 || event.sourceInterface.length !== 8) {
      fail(
        new Error(
          `malformed stock announce destination=${Buffer.from(event.destination).toString('hex')} ` +
            `interface=${Buffer.from(event.sourceInterface).toString('hex')}`
        )
      );
      return;
    }
    receivedFromStock = true;
  }
);

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
console.log('NAPI_ANNOUNCE_APP_DATA_PEER_UP');

const completionTimer = setInterval(async () => {
  if (stopping || !receivedFromStock) {
    return;
  }
  stopping = true;
  clearInterval(completionTimer);
  clearInterval(announceTimer);
  await node.stop();
  console.log('NAPI_ANNOUNCE_APP_DATA_OK received=1');
  process.exit(0);
}, 100);

setTimeout(() => {
  clearInterval(completionTimer);
  clearInterval(announceTimer);
  fail(new Error(`timeout receivedFromStock=${receivedFromStock}`));
}, 35000);
