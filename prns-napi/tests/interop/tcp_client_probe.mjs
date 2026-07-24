import { startNode } from '../../index.js';

const target = process.env.PRNS_TCP_TARGET;
if (!target) {
  console.error('FAILED PRNS_TCP_TARGET not set');
  process.exit(1);
}

let sent = false;

const node = startNode({}, (event) => {
  if (event.type !== 'announce' || sent) {
    return;
  }
  sent = true;
  console.log(`HEARD_HOST ${Buffer.from(event.destination).toString('hex')}`);
  node
    .sendSinglePacket(event.destination, Buffer.from('prns-napi-tcp-parity-ping'))
    .then((receipt) => {
      console.log(`PROVEN ${receipt.evidence}`);
      return node.stop();
    })
    .then(() => process.exit(0))
    .catch((error) => {
      console.error(`FAILED ${error.code ?? ''} ${error.message}`);
      process.exit(1);
    });
});
await node.ready();
await node.attachTcpClient({ target });
console.log('CLIENT_UP');

setTimeout(() => {
  console.error(`FAILED sent=${sent} proof never arrived`);
  process.exit(1);
}, 30000);
